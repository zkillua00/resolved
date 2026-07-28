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
    MouseButton, MouseDownEvent, ParentElement as _, PathPromptOptions, Pixels, Point, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Task, Timer, Window,
    anchored, deferred, div, prelude::FluentBuilder as _, px,
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

const TEMPLATE_HIGHLIGHT_DEBOUNCE: Duration = Duration::from_millis(90);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RequestTab {
    Headers,
    Body,
    PreRequest,
    PostResponse,
}

impl RequestTab {
    fn index(self) -> usize {
        match self {
            Self::Headers => 0,
            Self::Body => 1,
            Self::PreRequest => 2,
            Self::PostResponse => 3,
        }
    }

    fn from_index(index: usize) -> Self {
        match index {
            1 => Self::Body,
            2 => Self::PreRequest,
            3 => Self::PostResponse,
            _ => Self::Headers,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResponseTab {
    Body,
    Headers,
    Preview,
    Scripts,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScriptConsoleTone {
    Neutral,
    Info,
    Warning,
    Danger,
    Success,
    Debug,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ScriptConsoleRow {
    label: String,
    message: String,
    detail: Option<String>,
    copy_value: String,
    tone: ScriptConsoleTone,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ScriptConsoleSection {
    key: String,
    title: String,
    duration: Option<Duration>,
    rows: Vec<ScriptConsoleRow>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ScriptConsoleModel {
    sections: Vec<ScriptConsoleSection>,
}

impl ResponseTab {
    fn index(self) -> usize {
        match self {
            Self::Body => 0,
            Self::Headers => 1,
            Self::Preview => 2,
            Self::Scripts => 3,
        }
    }

    fn from_index(index: usize) -> Self {
        match index {
            1 => Self::Headers,
            2 => Self::Preview,
            3 => Self::Scripts,
            _ => Self::Body,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SidebarTab {
    Collections,
    Environments,
    History,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PendingDelete {
    History,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExecutionStage {
    PreRequest,
    Request,
    PostResponse,
}

impl ExecutionStage {
    fn label(self) -> &'static str {
        match self {
            Self::PreRequest => "Pre-script…",
            Self::Request => "Sending…",
            Self::PostResponse => "Post-script…",
        }
    }
}

struct HeaderRow {
    id: usize,
    name: Entity<InputState>,
    value: Entity<InputState>,
    enabled: bool,
    _subscriptions: Vec<Subscription>,
}

struct BodyFieldRow {
    id: usize,
    name: Entity<InputState>,
    value: Entity<InputState>,
    enabled: bool,
    kind: BodyFieldKind,
    _subscriptions: Vec<Subscription>,
}

struct EnvironmentVariableRow {
    id: String,
    key: Entity<InputState>,
    value: Entity<InputState>,
    enabled: bool,
    secret: bool,
    _subscriptions: Vec<Subscription>,
}

#[derive(Clone)]
enum TemplateVariableAction {
    Create,
    Enable { variable_id: String },
}

#[derive(Clone)]
struct TemplateVariablePopover {
    name: String,
    expected_environment_id: Option<String>,
    environment_name: Option<String>,
    action: TemplateVariableAction,
    value: Entity<InputState>,
    position: Point<Pixels>,
    error: Option<String>,
}

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
    body_fields: Vec<BodyFieldRow>,
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

// Script errors intentionally carry a full structured report and diagnostic.
// Execution happens off the UI thread, so preserving that context is more
// useful than boxing every result boundary.
#[allow(clippy::result_large_err)]
impl ApiTester {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let script_variable_catalog = ScriptVariableCatalog::default().shared();
        let template_variable_catalog = TemplateVariableCatalog::default().shared();
        let method = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("METHOD")
                .default_value("GET")
        });
        let url_template_catalog = Rc::clone(&template_variable_catalog);
        let url = cx.new(|cx| {
            template_input_state(
                window,
                cx,
                url_template_catalog,
                "https://api.example.com/users",
                "https://httpbin.org/get",
            )
        });
        let body_completion_catalog = Rc::clone(&template_variable_catalog);
        let body_hover_catalog = Rc::clone(&template_variable_catalog);
        let body = cx.new(|cx| {
            CodeEditor::new(
                CodeEditorConfig::default()
                    .language(CodeLanguage::Json)
                    .placeholder("Raw request body · ⌘F to search")
                    .rows(12)
                    .soft_wrap(false)
                    .format_action(true)
                    .completion_provider(Rc::new(TemplateCompletionProvider::new(
                        body_completion_catalog,
                    )))
                    .hover_provider(Rc::new(TemplateHoverProvider::new(body_hover_catalog))),
                window,
                cx,
            )
        });
        let pre_completion_catalog = Rc::clone(&script_variable_catalog);
        let pre_diagnostic_catalog = Rc::clone(&script_variable_catalog);
        let pre_request_script = cx.new(|cx| {
            let completion_catalog = Rc::clone(&pre_completion_catalog);
            let diagnostic_catalog = Rc::clone(&pre_diagnostic_catalog);
            let intelligence = Rc::new(ScriptCompletionProvider::new(
                ScriptEditorPhase::PreRequest,
                completion_catalog,
            ));
            CodeEditor::new(
                CodeEditorConfig::default()
                    .language(CodeLanguage::JavaScript)
                    .placeholder(
                        "api.request.headers.set(\"X-Token\", api.environment.get(\"token\"));",
                    )
                    .rows(12)
                    .soft_wrap(false)
                    .completion_provider(intelligence.clone())
                    .hover_provider(intelligence)
                    .diagnostic_provider(move |source| {
                        diagnostics_for_source(source, &diagnostic_catalog.borrow())
                            .into_iter()
                            .map(Into::into)
                            .collect()
                    }),
                window,
                cx,
            )
        });
        let post_completion_catalog = Rc::clone(&script_variable_catalog);
        let post_diagnostic_catalog = Rc::clone(&script_variable_catalog);
        let post_response_script = cx.new(|cx| {
            let completion_catalog = Rc::clone(&post_completion_catalog);
            let diagnostic_catalog = Rc::clone(&post_diagnostic_catalog);
            let intelligence = Rc::new(ScriptCompletionProvider::new(
                ScriptEditorPhase::PostResponse,
                completion_catalog,
            ));
            CodeEditor::new(
                CodeEditorConfig::default()
                    .language(CodeLanguage::JavaScript)
                    .placeholder(
                        "api.test(\"status is 200\", () => api.assert(api.response.status === 200));",
                    )
                    .rows(12)
                    .soft_wrap(false)
                    .completion_provider(intelligence.clone())
                    .hover_provider(intelligence)
                    .diagnostic_provider(move |source| {
                        diagnostics_for_source(source, &diagnostic_catalog.borrow())
                            .into_iter()
                            .map(Into::into)
                            .collect()
                    }),
                window,
                cx,
            )
        });
        let response_editor = cx.new(|cx| {
            CodeEditor::new(
                CodeEditorConfig::default()
                    .language(CodeLanguage::Json)
                    .placeholder("Response body")
                    .rows(20)
                    .soft_wrap(false)
                    .read_only(true),
                window,
                cx,
            )
        });
        let debug_overlay = cx.new(DebugOverlay::new);

        let database_store = DatabaseStore::default();
        let (
            history,
            workspace,
            history_warning,
            workspace_warning,
            history_writable,
            workspace_writable,
        ) = match database_store.initialize() {
            Ok(()) => {
                let import_warning = database_store
                    .import_legacy_if_needed()
                    .err()
                    .map(|error| format!("Legacy JSON data could not be imported: {error}"));
                let (history, history_warning, history_writable) =
                    match database_store.load_history() {
                        Ok(history) => (history, None, true),
                        Err(error) => (
                            RequestHistory::default(),
                            Some(format!(
                                "History could not be loaded and will not be overwritten: {error}"
                            )),
                            false,
                        ),
                    };
                let (workspace, workspace_load_warning, workspace_writable) =
                    match database_store.load_workspace() {
                        Ok(workspace) => (workspace, None, true),
                        Err(error) => (
                            Workspace::default(),
                            Some(format!(
                                "Workspace could not be loaded and will not be overwritten: {error}"
                            )),
                            false,
                        ),
                    };
                let workspace_warning = match (workspace_load_warning, import_warning) {
                    (Some(load), Some(import)) => Some(format!("{load}\n{import}")),
                    (Some(load), None) => Some(load),
                    (None, Some(import)) => Some(import),
                    (None, None) => None,
                };
                (
                    history,
                    workspace,
                    history_warning,
                    workspace_warning,
                    history_writable,
                    workspace_writable,
                )
            }
            Err(error) => (
                RequestHistory::default(),
                Workspace::default(),
                Some(format!(
                    "Database could not be opened and history will not be overwritten: {error}"
                )),
                Some(format!(
                    "Database could not be opened and workspace will not be overwritten: {error}"
                )),
                false,
                false,
            ),
        };
        let selected_collection_id = workspace
            .collections
            .first()
            .map(|collection| collection.id.clone());
        update_script_variable_catalog(&script_variable_catalog, &workspace);
        template_variable_catalog
            .borrow_mut()
            .replace_environment(workspace.active_environment());
        let selected_environment_id = workspace.active_environment_id.clone().or_else(|| {
            workspace
                .environments
                .first()
                .map(|environment| environment.id.clone())
        });
        let selected_collection_name = selected_collection_id
            .as_deref()
            .and_then(|id| workspace.collection(id))
            .map(|collection| collection.name.clone())
            .unwrap_or_default();
        let collection_search =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search collections"));
        let environment_search =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search environments"));
        let expanded_collection_ids = workspace
            .collections
            .iter()
            .map(|collection| collection.id.clone())
            .collect();
        let selected_environment_name = selected_environment_id
            .as_deref()
            .and_then(|id| workspace.environment(id))
            .map(|environment| environment.name.clone())
            .unwrap_or_default();
        let collection_name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Collection name")
                .default_value(selected_collection_name)
        });
        let collection_delete_confirmation =
            cx.new(|cx| InputState::new(window, cx).placeholder("Type the collection name"));
        let saved_request_name =
            cx.new(|cx| InputState::new(window, cx).placeholder("Request name"));
        let environment_name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Environment name")
                .default_value(selected_environment_name)
        });
        let environment_variables = Self::environment_rows(
            selected_environment_id
                .as_deref()
                .and_then(|id| workspace.environment(id)),
            window,
            cx,
        );

        let client = build_client().expect("failed to create the HTTP client");
        let runtime = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_name("api-tester-network")
                .enable_all()
                .build()
                .expect("failed to create the network runtime"),
        );

        let url_subscription = cx.subscribe_in(&url, window, |this, input, event, window, cx| {
            this.track_template_input_focus(input, event);
            if matches!(event, InputEvent::Change) {
                let input = this.url.clone();
                this.schedule_template_input_refresh(&input, cx);
                this.refresh_request_dirty_part(RequestDirtyPart::Url, cx);
            }
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.start_request(window, cx);
            }
        });
        let method_subscription = cx.subscribe_in(&method, window, |this, _, event, window, cx| {
            if matches!(event, InputEvent::Change) {
                this.refresh_request_dirty_part(RequestDirtyPart::Method, cx);
            }
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.start_request(window, cx);
            }
        });
        let body_subscription = cx.subscribe(&body, |this, editor, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                let input = editor.read(cx).input_state();
                this.schedule_template_input_refresh(&input, cx);
                this.refresh_request_dirty_part(RequestDirtyPart::RawBody, cx);
            }
        });
        let body_format_subscription =
            cx.subscribe_in(&body, window, |this, _, event, window, cx| {
                if matches!(event, CodeEditorEvent::FormatRequested) {
                    this.format_raw_body(window, cx);
                }
            });
        let pre_request_subscription =
            cx.subscribe(&pre_request_script, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.refresh_request_dirty_part(RequestDirtyPart::PreScript, cx);
                }
            });
        let post_response_subscription =
            cx.subscribe(&post_response_script, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.refresh_request_dirty_part(RequestDirtyPart::PostScript, cx);
                }
            });
        let collection_name_subscription =
            cx.subscribe_in(&collection_name, window, |this, _, event, _, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.rename_collection(cx);
                    this.renaming_collection_id = None;
                    cx.notify();
                }
            });

        let mut this = Self {
            method,
            url,
            body,
            pre_request_script,
            post_response_script,
            response_editor,
            headers: Vec::new(),
            next_header_id: 0,
            body_mode: BodyMode::Raw,
            raw_body_language: RawBodyLanguage::Json,
            body_fields: Vec::new(),
            next_body_field_id: 0,
            request_tab: RequestTab::Headers,
            response_tab: ResponseTab::Body,
            pretty_body: true,
            sending: false,
            execution_stage: None,
            request_generation: 0,
            abort_handle: None,
            script_cancellation: None,
            response: None,
            request_error: None,
            script_diagnostic: None,
            pre_script_report: None,
            post_script_report: None,
            preview_error: None,
            copied: false,
            client,
            runtime,
            history,
            history_warning,
            history_writable,
            workspace,
            database_store,
            workspace_warning,
            workspace_writable,
            sidebar_tab: SidebarTab::Collections,
            navigation_compact: false,
            selected_collection_id,
            active_saved_request_id: None,
            detached_request_dirty: false,
            request_dirty: RequestDirtyState::default(),
            loaded_request_baseline: RequestTemplate::default(),
            pending_request_load_key: None,
            request_notice: None,
            selected_environment_id,
            collection_search,
            environment_search,
            expanded_collection_ids,
            renaming_collection_id: None,
            collection_name,
            collection_delete_confirmation,
            saved_request_name,
            environment_name,
            environment_variables,
            next_variable_row_id: 0,
            pending_delete: None,
            script_variable_catalog,
            template_variable_catalog,
            template_highlight_tasks: HashMap::new(),
            template_variable_popover: None,
            focused_template_input: None,
            debug_overlay,
            preview: None,
            _subscriptions: vec![
                url_subscription,
                method_subscription,
                body_subscription,
                body_format_subscription,
                pre_request_subscription,
                post_response_subscription,
                collection_name_subscription,
            ],
        };
        this.push_header_row("", "", true, window, cx);
        this.refresh_variable_intelligence(cx);
        this.loaded_request_baseline = this.request_template(cx);
        this.request_dirty.clear();
        this
    }

    fn environment_rows(
        environment: Option<&Environment>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<EnvironmentVariableRow> {
        environment
            .map(|environment| {
                environment
                    .variables
                    .iter()
                    .map(|variable| {
                        let variable_id = variable.id.clone();
                        let key = variable.key.clone();
                        let value = variable.value.clone();
                        let key_state = cx.new(|cx| {
                            InputState::new(window, cx)
                                .placeholder("Variable")
                                .default_value(key)
                        });
                        let value_state = cx.new(|cx| {
                            InputState::new(window, cx)
                                .placeholder("Value")
                                .default_value(value)
                                .masked(variable.secret)
                        });
                        let key_row_id = variable_id.clone();
                        let key_subscription = cx.subscribe_in(
                            &key_state,
                            window,
                            move |this, _, event, window, cx| {
                                if matches!(event, InputEvent::PressEnter { .. })
                                    && let Some(row) = this
                                        .environment_variables
                                        .iter()
                                        .find(|row| row.id == key_row_id)
                                {
                                    row.value.read(cx).focus_handle(cx).focus(window);
                                }
                            },
                        );
                        let value_row_id = variable_id.clone();
                        let value_subscription = cx.subscribe_in(
                            &value_state,
                            window,
                            move |this, _, event, window, cx| {
                                if matches!(event, InputEvent::PressEnter { .. }) {
                                    this.focus_next_environment_row(&value_row_id, window, cx);
                                }
                            },
                        );
                        EnvironmentVariableRow {
                            id: variable_id,
                            key: key_state,
                            value: value_state,
                            enabled: variable.enabled,
                            secret: variable.secret,
                            _subscriptions: vec![key_subscription, value_subscription],
                        }
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn push_environment_row(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = format!("draft-variable-{}", self.next_variable_row_id);
        self.next_variable_row_id = self.next_variable_row_id.wrapping_add(1);
        let key = cx.new(|cx| InputState::new(window, cx).placeholder("Variable"));
        let value = cx.new(|cx| InputState::new(window, cx).placeholder("Value"));
        let key_row_id = id.clone();
        let key_subscription = cx.subscribe_in(&key, window, move |this, _, event, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. })
                && let Some(row) = this
                    .environment_variables
                    .iter()
                    .find(|row| row.id == key_row_id)
            {
                row.value.read(cx).focus_handle(cx).focus(window);
            }
        });
        let value_row_id = id.clone();
        let value_subscription =
            cx.subscribe_in(&value, window, move |this, _, event, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.focus_next_environment_row(&value_row_id, window, cx);
                }
            });
        self.environment_variables.push(EnvironmentVariableRow {
            id,
            key,
            value,
            enabled: true,
            secret: false,
            _subscriptions: vec![key_subscription, value_subscription],
        });
        cx.notify();
    }

    fn push_header_row(
        &mut self,
        name: impl Into<SharedString>,
        value: impl Into<SharedString>,
        enabled: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = name.into();
        let value = value.into();
        let id = self.next_header_id;
        self.next_header_id = self.next_header_id.wrapping_add(1);
        let name_catalog = Rc::clone(&self.template_variable_catalog);
        let name_state = cx.new(|cx| template_input_state(window, cx, name_catalog, "Key", name));
        let value_catalog = Rc::clone(&self.template_variable_catalog);
        let value_state =
            cx.new(|cx| template_input_state(window, cx, value_catalog, "Value", value));
        let name_template_input = name_state.clone();
        let name_subscription = cx.subscribe_in(
            &name_state,
            window,
            move |this, input, event, window, cx| {
                this.track_template_input_focus(input, event);
                if matches!(event, InputEvent::Change) {
                    this.schedule_template_input_refresh(&name_template_input, cx);
                    this.refresh_request_dirty_part(RequestDirtyPart::Headers, cx);
                }
                if matches!(event, InputEvent::PressEnter { .. })
                    && let Some(row) = this.headers.iter().find(|row| row.id == id)
                {
                    row.value.read(cx).focus_handle(cx).focus(window);
                }
            },
        );
        let value_template_input = value_state.clone();
        let value_subscription = cx.subscribe_in(
            &value_state,
            window,
            move |this, input, event, window, cx| {
                this.track_template_input_focus(input, event);
                if matches!(event, InputEvent::Change) {
                    this.schedule_template_input_refresh(&value_template_input, cx);
                    this.refresh_request_dirty_part(RequestDirtyPart::Headers, cx);
                }
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.focus_next_header_row(id, window, cx);
                }
            },
        );
        self.refresh_template_input(&name_state, cx);
        self.refresh_template_input(&value_state, cx);
        self.headers.push(HeaderRow {
            id,
            name: name_state,
            value: value_state,
            enabled,
            _subscriptions: vec![name_subscription, value_subscription],
        });
        self.refresh_request_dirty_part(RequestDirtyPart::Headers, cx);
        if !self.request_dirty.is_hydrating() {
            cx.notify();
        }
    }

    fn focus_next_header_row(
        &mut self,
        row_id: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.headers.iter().position(|row| row.id == row_id) else {
            return;
        };
        if index + 1 == self.headers.len() {
            self.push_header_row("", "", true, window, cx);
        }
        if let Some(input) = self.headers.get(index + 1).map(|row| row.name.clone()) {
            input.read(cx).focus_handle(cx).focus(window);
        }
    }

    fn duplicate_header_row(&mut self, row_id: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.headers.iter().position(|row| row.id == row_id) else {
            return;
        };
        let name = self.headers[index].name.read(cx).value();
        let value = self.headers[index].value.read(cx).value();
        let enabled = self.headers[index].enabled;
        self.push_header_row(name, value, enabled, window, cx);
        if let Some(duplicate) = self.headers.pop() {
            self.headers.insert(index + 1, duplicate);
        }
        cx.notify();
    }

    fn copy_header_row(&mut self, row_id: usize, cx: &mut Context<Self>) {
        let Some(row) = self.headers.iter().find(|row| row.id == row_id) else {
            return;
        };
        let name = row.name.read(cx).value();
        let value = row.value.read(cx).value();
        let header = if name.trim().is_empty() {
            value.to_string()
        } else {
            format!("{name}: {value}")
        };
        cx.write_to_clipboard(ClipboardItem::new_string(header));
        self.request_notice = Some("Copied header.".to_owned());
        cx.notify();
    }

    fn remove_header_row(&mut self, row_id: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.headers.retain(|row| row.id != row_id);
        if self.headers.is_empty() {
            self.push_header_row("", "", true, window, cx);
        }
        self.refresh_request_dirty_part(RequestDirtyPart::Headers, cx);
        cx.notify();
    }

    fn focus_next_environment_row(
        &mut self,
        row_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self
            .environment_variables
            .iter()
            .position(|row| row.id == row_id)
        else {
            return;
        };
        if index + 1 == self.environment_variables.len() {
            self.push_environment_row(window, cx);
        }
        if let Some(input) = self
            .environment_variables
            .get(index + 1)
            .map(|row| row.key.clone())
        {
            input.read(cx).focus_handle(cx).focus(window);
        }
    }

    fn duplicate_environment_row(
        &mut self,
        row_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self
            .environment_variables
            .iter()
            .position(|row| row.id == row_id)
        else {
            return;
        };
        let key = self.environment_variables[index]
            .key
            .read(cx)
            .value()
            .to_string();
        let value = self.environment_variables[index]
            .value
            .read(cx)
            .unmask_value()
            .to_string();
        let enabled = self.environment_variables[index].enabled;
        let secret = self.environment_variables[index].secret;

        self.push_environment_row(window, cx);
        let Some(mut duplicate) = self.environment_variables.pop() else {
            return;
        };
        duplicate.enabled = enabled;
        duplicate.secret = secret;
        duplicate
            .key
            .update(cx, |input, cx| input.set_value(key, window, cx));
        duplicate.value.update(cx, |input, cx| {
            input.set_value(value, window, cx);
            input.set_masked(secret, window, cx);
        });
        let input = duplicate.key.clone();
        self.environment_variables.insert(index + 1, duplicate);
        input.read(cx).focus_handle(cx).focus(window);
        cx.notify();
    }

    fn push_body_field_row(
        &mut self,
        name: impl Into<SharedString>,
        value: impl Into<SharedString>,
        enabled: bool,
        kind: BodyFieldKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = name.into();
        let value = value.into();
        let id = self.next_body_field_id;
        self.next_body_field_id = self.next_body_field_id.wrapping_add(1);
        let name_catalog = Rc::clone(&self.template_variable_catalog);
        let name_state = cx.new(|cx| template_input_state(window, cx, name_catalog, "Key", name));
        let value_catalog = Rc::clone(&self.template_variable_catalog);
        let value_placeholder = if kind == BodyFieldKind::File {
            "Choose or enter a file path"
        } else {
            "Value"
        };
        let value_state =
            cx.new(|cx| template_input_state(window, cx, value_catalog, value_placeholder, value));
        let name_template_input = name_state.clone();
        let name_subscription = cx.subscribe_in(
            &name_state,
            window,
            move |this, input, event, window, cx| {
                this.track_template_input_focus(input, event);
                if matches!(event, InputEvent::Change) {
                    this.schedule_template_input_refresh(&name_template_input, cx);
                    this.refresh_request_dirty_part(RequestDirtyPart::BodyFields, cx);
                }
                if matches!(event, InputEvent::PressEnter { .. })
                    && let Some(row) = this.body_fields.iter().find(|row| row.id == id)
                {
                    row.value.read(cx).focus_handle(cx).focus(window);
                }
            },
        );
        let value_template_input = value_state.clone();
        let value_subscription = cx.subscribe_in(
            &value_state,
            window,
            move |this, input, event, window, cx| {
                this.track_template_input_focus(input, event);
                if matches!(event, InputEvent::Change) {
                    this.schedule_template_input_refresh(&value_template_input, cx);
                    this.refresh_request_dirty_part(RequestDirtyPart::BodyFields, cx);
                }
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.focus_next_body_field_row(id, window, cx);
                }
            },
        );
        self.refresh_template_input(&name_state, cx);
        self.refresh_template_input(&value_state, cx);
        self.body_fields.push(BodyFieldRow {
            id,
            name: name_state,
            value: value_state,
            enabled,
            kind,
            _subscriptions: vec![name_subscription, value_subscription],
        });
        self.refresh_request_dirty_part(RequestDirtyPart::BodyFields, cx);
        if !self.request_dirty.is_hydrating() {
            cx.notify();
        }
    }

    fn focus_next_body_field_row(
        &mut self,
        row_id: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.body_fields.iter().position(|row| row.id == row_id) else {
            return;
        };
        if index + 1 == self.body_fields.len() {
            self.push_body_field_row("", "", true, BodyFieldKind::Text, window, cx);
        }
        if let Some(input) = self.body_fields.get(index + 1).map(|row| row.name.clone()) {
            input.read(cx).focus_handle(cx).focus(window);
        }
    }

    fn duplicate_body_field_row(
        &mut self,
        row_id: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.body_fields.iter().position(|row| row.id == row_id) else {
            return;
        };
        let name = self.body_fields[index].name.read(cx).value();
        let value = self.body_fields[index].value.read(cx).value();
        let enabled = self.body_fields[index].enabled;
        let kind = self.body_fields[index].kind;
        self.push_body_field_row(name, value, enabled, kind, window, cx);
        if let Some(duplicate) = self.body_fields.pop() {
            self.body_fields.insert(index + 1, duplicate);
        }
        cx.notify();
    }

    fn remove_body_field_row(
        &mut self,
        row_id: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.body_fields.retain(|row| row.id != row_id);
        if self.body_fields.is_empty() {
            self.push_body_field_row("", "", true, BodyFieldKind::Text, window, cx);
        }
        self.refresh_request_dirty_part(RequestDirtyPart::BodyFields, cx);
        cx.notify();
    }

    fn set_body_field_kind(
        &mut self,
        row_id: usize,
        kind: BodyFieldKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(row) = self.body_fields.iter_mut().find(|row| row.id == row_id) else {
            return;
        };
        row.kind = kind;
        row.value.update(cx, |state, cx| {
            state.set_placeholder(
                if kind == BodyFieldKind::File {
                    "Choose or enter a file path"
                } else {
                    "Value"
                },
                window,
                cx,
            );
        });
        self.refresh_request_dirty_part(RequestDirtyPart::BodyFields, cx);
        cx.notify();
    }

    fn draft(&self, cx: &App) -> RequestDraft {
        let method = self.method.read(cx).value().trim().to_ascii_uppercase();
        let headers = self
            .headers
            .iter()
            .map(|row| {
                let mut header = HeaderEntry::new(
                    row.name.read(cx).value().to_string(),
                    row.value.read(cx).value().to_string(),
                );
                header.enabled = row.enabled;
                header
            })
            .collect();

        let mut draft = RequestDraft::new(method, self.url.read(cx).value().to_string());
        draft.headers = headers;
        draft.body = self.body.read(cx).value(cx).to_string();
        draft.body_mode = self.body_mode;
        draft.raw_body_language = self.raw_body_language;
        draft.body_fields = self
            .body_fields
            .iter()
            .map(|row| BodyField {
                enabled: row.enabled,
                name: row.name.read(cx).value().to_string(),
                value: row.value.read(cx).value().to_string(),
                kind: row.kind,
            })
            .collect();
        draft
    }

    fn request_template(&self, cx: &App) -> RequestTemplate {
        RequestTemplate {
            request: self.draft(cx),
            scripts: RequestScripts {
                pre_request: self.pre_request_script.read(cx).value(cx).to_string(),
                post_response: self.post_response_script.read(cx).value(cx).to_string(),
            },
        }
    }

    fn request_is_dirty(&self) -> bool {
        self.detached_request_dirty || self.request_dirty.any()
    }

    fn refresh_request_dirty_part(&mut self, part: RequestDirtyPart, cx: &mut Context<Self>) {
        if self.request_dirty.is_hydrating() {
            return;
        }

        let part_is_dirty = self.request_part_is_dirty(part, cx);
        self.request_dirty.set(part, part_is_dirty);
    }

    fn request_part_is_dirty(&self, part: RequestDirtyPart, cx: &App) -> bool {
        let baseline = &self.loaded_request_baseline;
        match part {
            RequestDirtyPart::Method => {
                let current = self.method.read(cx).value();
                !current
                    .trim()
                    .eq_ignore_ascii_case(baseline.request.method.as_str())
            }
            RequestDirtyPart::Url => {
                !input_text_equals(&self.url, baseline.request.url.as_str(), cx)
            }
            RequestDirtyPart::Headers => {
                self.headers.len() != baseline.request.headers.len()
                    || self
                        .headers
                        .iter()
                        .zip(&baseline.request.headers)
                        .any(|(current, saved)| {
                            current.enabled != saved.enabled
                                || !input_text_equals(&current.name, saved.name.as_str(), cx)
                                || !input_text_equals(&current.value, saved.value.as_str(), cx)
                        })
            }
            RequestDirtyPart::RawBody => {
                let input = self.body.read(cx).input_state();
                !input_text_equals(&input, baseline.request.body.as_str(), cx)
            }
            RequestDirtyPart::BodyMode => self.body_mode != baseline.request.body_mode,
            RequestDirtyPart::RawBodyLanguage => {
                self.raw_body_language != baseline.request.raw_body_language
            }
            RequestDirtyPart::BodyFields => {
                self.body_fields.len() != baseline.request.body_fields.len()
                    || self
                        .body_fields
                        .iter()
                        .zip(&baseline.request.body_fields)
                        .any(|(current, saved)| {
                            current.enabled != saved.enabled
                                || current.kind != saved.kind
                                || !input_text_equals(&current.name, saved.name.as_str(), cx)
                                || !input_text_equals(&current.value, saved.value.as_str(), cx)
                        })
            }
            RequestDirtyPart::PreScript => {
                let input = self.pre_request_script.read(cx).input_state();
                !input_text_equals(&input, baseline.scripts.pre_request.as_str(), cx)
            }
            RequestDirtyPart::PostScript => {
                let input = self.post_response_script.read(cx).input_state();
                !input_text_equals(&input, baseline.scripts.post_response.as_str(), cx)
            }
        }
    }

    fn active_environment_editor_is_dirty(&self, cx: &App) -> bool {
        self.workspace.active_environment_id == self.selected_environment_id
            && self.environment_editor_is_dirty(cx)
    }

    fn script_scope(environment: Option<&Environment>) -> ScriptScope {
        let mut script_environment = ScriptEnvironment::default();
        if let Some(environment) = environment {
            for variable in environment
                .variables
                .iter()
                .filter(|variable| variable.enabled)
            {
                if variable.secret {
                    script_environment.insert_secret(variable.key.clone(), variable.value.clone());
                } else {
                    script_environment.insert(variable.key.clone(), variable.value.clone());
                }
            }
        }
        ScriptScope {
            environment: script_environment,
            ..ScriptScope::default()
        }
    }

    fn refresh_variable_intelligence(&mut self, cx: &mut Context<Self>) {
        self.template_highlight_tasks.clear();
        update_script_variable_catalog(&self.script_variable_catalog, &self.workspace);
        self.template_variable_catalog
            .borrow_mut()
            .replace_environment(self.workspace.active_environment());
        self.pre_request_script
            .update(cx, |editor, cx| editor.refresh_diagnostics(cx));
        self.post_response_script
            .update(cx, |editor, cx| editor.refresh_diagnostics(cx));

        let mut inputs =
            Vec::with_capacity(2 + self.headers.len() * 2 + self.body_fields.len() * 2);
        inputs.push(self.url.clone());
        inputs.push(self.body.read(cx).input_state());
        for row in &self.headers {
            inputs.push(row.name.clone());
            inputs.push(row.value.clone());
        }
        for row in &self.body_fields {
            inputs.push(row.name.clone());
            inputs.push(row.value.clone());
        }
        for input in inputs {
            self.refresh_template_input(&input, cx);
        }
    }

    fn refresh_template_input(&self, input: &Entity<InputState>, cx: &mut Context<Self>) {
        let source = input.read(cx).text().to_string();
        let colors = TemplateHighlightColors {
            valid: primary_lavender(),
            warning: cx.theme().warning,
            error: cx.theme().red,
        };
        let catalog = self.template_variable_catalog.borrow();
        let semantic_highlights = semantic_style_spans(&source, &catalog, colors);
        drop(catalog);

        input.update(cx, |input, cx| {
            input.set_semantic_highlights(semantic_highlights, cx);
        });
    }

    fn schedule_template_input_refresh(
        &mut self,
        input: &Entity<InputState>,
        cx: &mut Context<Self>,
    ) {
        if self.request_dirty.is_hydrating() {
            return;
        }
        let input_id = input.entity_id();
        let input = input.downgrade();
        let task = cx.spawn(async move |this, cx| {
            Timer::after(TEMPLATE_HIGHLIGHT_DEBOUNCE).await;
            let (Some(this), Some(input)) = (this.upgrade(), input.upgrade()) else {
                return;
            };
            this.update(cx, |this, cx| {
                this.refresh_template_input(&input, cx);
            })
            .ok();
        });
        self.template_highlight_tasks.insert(input_id, task);
    }

    fn focused_single_line_template_input(
        &self,
        window: &Window,
        cx: &App,
    ) -> Option<Entity<InputState>> {
        self.focused_template_input
            .as_ref()
            .filter(|input| input.read(cx).focus_handle(cx).is_focused(window))
            .cloned()
    }

    fn track_template_input_focus(&mut self, input: &Entity<InputState>, event: &InputEvent) {
        match event {
            InputEvent::Focus => self.focused_template_input = Some(input.clone()),
            InputEvent::Blur
                if self
                    .focused_template_input
                    .as_ref()
                    .is_some_and(|focused| focused.entity_id() == input.entity_id()) =>
            {
                self.focused_template_input = None;
            }
            _ => {}
        }
    }

    fn capture_template_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.is_held {
            return;
        }
        let modifiers = event.keystroke.modifiers;
        if modifiers.platform || modifiers.control || modifiers.function {
            return;
        }
        let Some(typed) = event.keystroke.key_char.as_deref() else {
            return;
        };
        if typed.chars().count() != 1 {
            return;
        }
        if !matches!(typed, "{" | "}") {
            return;
        }
        let Some(input) = self.focused_single_line_template_input(window, cx) else {
            return;
        };
        let handled = input.update(cx, |input, cx| {
            if EntityInputHandler::marked_text_range(input, window, cx).is_some() {
                return false;
            }
            apply_template_pair_edit(input, typed, window, cx)
        });
        if handled {
            cx.stop_propagation();
        }
    }

    fn open_template_variable_popover(
        &mut self,
        input: Entity<InputState>,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.button != MouseButton::Left {
            return;
        }
        let Some(clicked_utf16) = input.update(cx, |input, cx| {
            EntityInputHandler::character_index_for_point(input, event.position, window, cx)
        }) else {
            self.template_variable_popover = None;
            cx.notify();
            return;
        };
        let source = input.read(cx).text().to_string();
        let clicked_offset = input.read(cx).text().offset_utf16_to_offset(clicked_utf16);
        let Some(span) = scan_template_spans(&source).into_iter().find(|span| {
            span.complete && span.range.start <= clicked_offset && clicked_offset < span.range.end
        }) else {
            self.template_variable_popover = None;
            cx.notify();
            return;
        };
        let name = span.name(&source).to_owned();
        let catalog = self.template_variable_catalog.borrow();
        let action = match span.classification(&source, &catalog) {
            TemplateClassification::Missing => TemplateVariableAction::Create,
            TemplateClassification::Disabled => {
                let Some(variable) = catalog.variable(&name) else {
                    return;
                };
                TemplateVariableAction::Enable {
                    variable_id: variable.id.clone(),
                }
            }
            TemplateClassification::Available | TemplateClassification::Invalid(_) => {
                drop(catalog);
                self.template_variable_popover = None;
                cx.notify();
                return;
            }
        };
        let expected_environment_id = catalog.environment_id().map(ToOwned::to_owned);
        let environment_name = catalog.environment_name().map(ToOwned::to_owned);
        drop(catalog);

        let value = cx.new(|cx| InputState::new(window, cx).placeholder("Variable value"));
        let should_focus_value =
            matches!(action, TemplateVariableAction::Create) && expected_environment_id.is_some();
        self.template_variable_popover = Some(TemplateVariablePopover {
            name,
            expected_environment_id,
            environment_name,
            action,
            value: value.clone(),
            position: event.position,
            error: None,
        });
        if should_focus_value {
            value.read(cx).focus_handle(cx).focus(window);
        }
        cx.notify();
    }

    fn template_variable_mutation_blocker(
        &self,
        popover: &TemplateVariablePopover,
        cx: &App,
    ) -> Option<String> {
        let Some(expected_environment_id) = popover.expected_environment_id.as_deref() else {
            return Some("Select an active environment before creating variables.".to_owned());
        };
        if self.workspace.active_environment_id.as_deref() != Some(expected_environment_id) {
            return Some(
                "The active environment changed. Close this popover and try again.".to_owned(),
            );
        }
        if !self.workspace_writable {
            return Some("Environment storage is read-only for this session.".to_owned());
        }
        if self.sending {
            return Some(
                "Wait for the current request to finish before changing variables.".to_owned(),
            );
        }
        if self.active_environment_editor_is_dirty(cx) {
            return Some("Save or revert the active environment's pending edits first.".to_owned());
        }
        None
    }

    fn apply_template_variable_action(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(popover) = self.template_variable_popover.clone() else {
            return;
        };
        if self
            .template_variable_mutation_blocker(&popover, cx)
            .is_some()
        {
            if let Some(current) = self.template_variable_popover.as_mut() {
                // Mutation blockers are derived from live application state and
                // rendered directly by the popover. Do not cache one as a
                // persistence error or it can outlive the condition that caused it.
                current.error = None;
            }
            cx.notify();
            return;
        }
        let environment_id = popover
            .expected_environment_id
            .as_deref()
            .expect("mutation blocker requires an active environment");
        let mut candidate = self.workspace.clone();
        let result = match &popover.action {
            TemplateVariableAction::Create => candidate
                .add_environment_variable(
                    environment_id,
                    popover.name.clone(),
                    popover.value.read(cx).value().to_string(),
                    true,
                    false,
                )
                .map(|_| ()),
            TemplateVariableAction::Enable { variable_id } => {
                let variable = candidate
                    .environment(environment_id)
                    .and_then(|environment| {
                        environment
                            .variables
                            .iter()
                            .find(|variable| variable.id == *variable_id)
                    })
                    .cloned();
                variable.map_or_else(
                    || {
                        Err(crate::core::WorkspaceMutationError::NotFound {
                            kind: "variable",
                            id: variable_id.clone(),
                        })
                    },
                    |variable| {
                        candidate.update_environment_variable(
                            environment_id,
                            variable_id,
                            variable.key,
                            variable.value,
                            true,
                            variable.secret,
                        )
                    },
                )
            }
        };
        if let Err(error) = result {
            if let Some(current) = self.template_variable_popover.as_mut() {
                current.error = Some(error.to_string());
            }
            cx.notify();
            return;
        }
        if let Err(error) = self.commit_workspace(candidate) {
            if let Some(current) = self.template_variable_popover.as_mut() {
                current.error = Some(error);
            }
            cx.notify();
            return;
        }

        if self.selected_environment_id.as_deref() == Some(environment_id) {
            self.reload_environment_editor(window, cx);
        }
        self.refresh_variable_intelligence(cx);
        self.request_notice = Some(match popover.action {
            TemplateVariableAction::Create => {
                format!("Created environment variable '{}'.", popover.name)
            }
            TemplateVariableAction::Enable { .. } => {
                format!("Enabled environment variable '{}'.", popover.name)
            }
        });
        self.template_variable_popover = None;
        cx.notify();
    }

    fn close_template_variable_popover(&mut self, cx: &mut Context<Self>) {
        self.template_variable_popover = None;
        cx.notify();
    }

    fn open_environments_from_template(&mut self, cx: &mut Context<Self>) {
        self.sidebar_tab = SidebarTab::Environments;
        self.template_variable_popover = None;
        cx.notify();
    }

    fn start_request(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sending {
            return;
        }

        let validation_error = if input_text_is_blank(&self.method, cx) {
            Some("HTTP method cannot be empty.".to_owned())
        } else if self.active_environment_editor_is_dirty(cx) {
            Some(
                "The active environment has unsaved changes. Save or Revert them before sending."
                    .to_owned(),
            )
        } else {
            None
        };
        if let Some(error) = validation_error {
            self.response = None;
            self.request_error = Some(error);
            self.script_diagnostic = None;
            self.execution_stage = None;
            self.hide_preview(cx);
            cx.notify();
            return;
        }

        self.pending_request_load_key = None;
        self.request_notice = None;
        let template = self.request_template(cx);
        let environment_id = self.workspace.active_environment_id.clone();
        let scope = Self::script_scope(
            environment_id
                .as_deref()
                .and_then(|id| self.workspace.environment(id)),
        );
        self.request_generation = self.request_generation.wrapping_add(1);
        let generation = self.request_generation;
        self.sending = true;
        self.execution_stage = Some(ExecutionStage::PreRequest);
        self.response = None;
        self.request_error = None;
        self.script_diagnostic = None;
        self.pre_script_report = None;
        self.post_script_report = None;
        self.preview_error = None;
        self.copied = false;
        self.hide_preview(cx);

        let source = template.scripts.pre_request.clone();
        let request = template.request.clone();
        let cancellation = ScriptCancellation::new();
        self.script_cancellation = Some(cancellation.clone());
        let task = self
            .runtime
            .spawn_blocking(move || execute_pre_request(&source, &request, &scope, &cancellation));
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.finish_pre_request(generation, template, environment_id, result, window, cx);
            });
        })
        .detach();
    }

    fn finish_pre_request(
        &mut self,
        generation: u64,
        template: RequestTemplate,
        environment_id: Option<String>,
        result: Result<Result<PreRequestResult, ScriptError>, tokio::task::JoinError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if generation != self.request_generation {
            return;
        }

        self.script_cancellation = None;
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                self.fail_request(
                    &template.request,
                    format!("pre-request script task failed: {error}"),
                    cx,
                );
                return;
            }
        };
        let pre_result = match result {
            Ok(result) => result,
            Err(error) => {
                if matches!(
                    error.diagnostic.kind,
                    crate::core::ScriptErrorKind::Cancelled
                ) {
                    self.finish_cancelled(cx);
                    return;
                }
                self.pre_script_report = Some(error.report.clone());
                self.script_diagnostic = Some(error.diagnostic.clone());
                self.response_tab = ResponseTab::Scripts;
                self.fail_request(&template.request, error.to_string(), cx);
                return;
            }
        };

        self.pre_script_report = Some(pre_result.report.clone());
        if let Err(error) = self.apply_environment_mutations(
            environment_id.as_deref(),
            &pre_result.environment_mutations,
            window,
            cx,
        ) {
            self.fail_request(&template.request, error, cx);
            return;
        }

        let environment = environment_id
            .as_deref()
            .and_then(|id| self.workspace.environment(id));
        let mut resolved = match resolve_request(&pre_result.request, environment) {
            Ok(resolved) => resolved,
            Err(error) => {
                self.fail_request(&template.request, error.to_string(), cx);
                return;
            }
        };
        if let Some(environment) = environment {
            resolved.sensitive_values.extend(
                environment
                    .variables
                    .iter()
                    .filter(|variable| variable.enabled && variable.secret)
                    .map(|variable| variable.value.clone()),
            );
        }
        self.begin_network_request(
            generation,
            template,
            environment_id,
            resolved,
            pre_result.report,
            window,
            cx,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn begin_network_request(
        &mut self,
        generation: u64,
        template: RequestTemplate,
        environment_id: Option<String>,
        resolved: crate::core::ResolvedRequest,
        pre_report: ScriptReport,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execution_stage = Some(ExecutionStage::Request);
        let task: RequestTask = spawn_request(
            self.runtime.handle(),
            self.client.clone(),
            resolved.request.clone(),
        );
        self.abort_handle = Some(task.abort_handle());
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let result = task.wait().await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.finish_network_request(
                    generation,
                    template,
                    environment_id,
                    resolved,
                    pre_report,
                    result,
                    window,
                    cx,
                );
            });
        })
        .detach();
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_network_request(
        &mut self,
        generation: u64,
        template: RequestTemplate,
        environment_id: Option<String>,
        resolved: crate::core::ResolvedRequest,
        pre_report: ScriptReport,
        result: Result<ResponseData, RequestError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if generation != self.request_generation {
            return;
        }
        self.abort_handle = None;

        let response = match result {
            Ok(response) => response,
            Err(RequestError::Cancelled) => {
                self.finish_cancelled(cx);
                return;
            }
            Err(error) => {
                self.pre_script_report = Some(pre_report);
                self.fail_request_with_secrets(
                    &resolved.request,
                    resolved.redact_secrets(&error.to_string()),
                    &resolved.sensitive_values,
                    cx,
                );
                return;
            }
        };

        let mut display_response = response.clone();
        display_response.final_url = resolved.redact_secrets(&response.final_url);
        self.response = Some(display_response.clone());
        self.update_response_editor(&display_response, window, cx);

        self.execution_stage = Some(ExecutionStage::PostResponse);
        let scope = Self::script_scope(
            environment_id
                .as_deref()
                .and_then(|id| self.workspace.environment(id)),
        );
        let source = template.scripts.post_response.clone();
        let request = resolved.request.clone();
        let history_request = resolved.request.clone();
        let history_sensitive_values = resolved.sensitive_values.clone();
        let cancellation = ScriptCancellation::new();
        self.script_cancellation = Some(cancellation.clone());
        let task = self.runtime.spawn_blocking(move || {
            execute_post_response(&source, &request, &response, &scope, &cancellation)
        });
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.finish_post_response(
                    generation,
                    environment_id,
                    pre_report,
                    display_response,
                    history_request,
                    history_sensitive_values,
                    result,
                    window,
                    cx,
                );
            });
        })
        .detach();
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_post_response(
        &mut self,
        generation: u64,
        environment_id: Option<String>,
        pre_report: ScriptReport,
        response: ResponseData,
        history_request: RequestDraft,
        history_sensitive_values: Vec<String>,
        result: Result<Result<PostResponseResult, ScriptError>, tokio::task::JoinError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if generation != self.request_generation {
            return;
        }

        self.script_cancellation = None;
        self.sending = false;
        self.execution_stage = None;
        self.pre_script_report = Some(pre_report);
        self.response = Some(response.clone());

        match result {
            Ok(Ok(post_result)) => {
                let mutation_result = self.apply_environment_mutations(
                    environment_id.as_deref(),
                    &post_result.environment_mutations,
                    window,
                    cx,
                );
                self.post_script_report = Some(post_result.report);
                match mutation_result {
                    Ok(()) => {
                        self.script_diagnostic = None;
                        self.request_error = None;
                    }
                    Err(error) => {
                        self.request_error = Some(error);
                        self.response_tab = ResponseTab::Scripts;
                        self.hide_preview(cx);
                    }
                }
            }
            Ok(Err(error)) => {
                if matches!(
                    error.diagnostic.kind,
                    crate::core::ScriptErrorKind::Cancelled
                ) {
                    self.request_error =
                        Some("Request completed; post-response script cancelled".to_owned());
                } else {
                    self.request_error = Some(error.to_string());
                }
                self.post_script_report = Some(error.report);
                self.script_diagnostic = Some(error.diagnostic);
                self.response_tab = ResponseTab::Scripts;
                self.hide_preview(cx);
            }
            Err(error) => {
                self.request_error = Some(format!("post-response script task failed: {error}"));
                self.response_tab = ResponseTab::Scripts;
                self.hide_preview(cx);
            }
        }

        self.history.push(HistoryEntry::completed_with_secrets(
            &history_request,
            &response,
            &history_sensitive_values,
        ));
        self.persist_history();
        if self.response_tab == ResponseTab::Preview {
            self.show_preview(window, cx);
        }
        cx.notify();
    }

    fn cancel_request(&mut self, cx: &mut Context<Self>) {
        if let Some(cancellation) = self.script_cancellation.take() {
            cancellation.cancel();
        }
        if let Some(abort_handle) = self.abort_handle.take() {
            abort_handle.abort();
        }
        self.request_generation = self.request_generation.wrapping_add(1);
        self.finish_cancelled(cx);
    }

    fn finish_cancelled(&mut self, cx: &mut Context<Self>) {
        self.sending = false;
        self.execution_stage = None;
        self.abort_handle = None;
        self.script_cancellation = None;
        self.request_error = Some("Request cancelled".to_owned());
        self.preview_error = None;
        self.hide_preview(cx);
        cx.notify();
    }

    fn fail_request(&mut self, request: &RequestDraft, message: String, cx: &mut Context<Self>) {
        self.fail_request_with_secrets(request, message, &[], cx);
    }

    fn fail_request_with_secrets(
        &mut self,
        request: &RequestDraft,
        message: String,
        sensitive_values: &[String],
        cx: &mut Context<Self>,
    ) {
        self.sending = false;
        self.execution_stage = None;
        self.abort_handle = None;
        self.script_cancellation = None;
        self.history.push(HistoryEntry::failed_with_secrets(
            request,
            message.clone(),
            sensitive_values,
        ));
        self.request_error = Some(message);
        self.persist_history();
        cx.notify();
    }

    fn persist_history(&mut self) {
        if !self.history_writable {
            self.history_warning.get_or_insert_with(|| {
                "History is read-only because it could not be loaded safely.".to_owned()
            });
            return;
        }
        self.history_warning = self
            .database_store
            .save_history(&self.history)
            .err()
            .map(|error| format!("History could not be saved: {error}"));
    }

    fn commit_workspace(&mut self, candidate: Workspace) -> Result<(), String> {
        if !self.workspace_writable {
            return Err("Database is read-only for this session.".to_owned());
        }
        match self.database_store.save_workspace(&candidate) {
            Ok(()) => {
                self.workspace = candidate;
                self.workspace_warning = None;
                Ok(())
            }
            Err(error) => {
                let message = format!("Workspace could not be saved: {error}");
                self.workspace_warning = Some(message.clone());
                Err(message)
            }
        }
    }

    fn apply_environment_mutations(
        &mut self,
        environment_id: Option<&str>,
        mutations: &[EnvironmentMutation],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if mutations.is_empty() {
            return Ok(());
        }

        let Some(environment_id) = environment_id else {
            self.workspace_warning = Some(
                "Script environment changes were transient because no environment is active."
                    .to_owned(),
            );
            return Ok(());
        };
        if !self.workspace_writable {
            return Err(
                "Script environment changes could not be saved because storage is read-only."
                    .to_owned(),
            );
        }

        let mut candidate = self.workspace.clone();
        let original_metadata = candidate
            .environment(environment_id)
            .ok_or_else(|| format!("active environment '{environment_id}' no longer exists"))?
            .variables
            .iter()
            .map(|variable| {
                (
                    variable.key.clone(),
                    (variable.id.clone(), variable.enabled, variable.secret),
                )
            })
            .collect::<BTreeMap<_, _>>();
        for mutation in mutations {
            let result = match mutation {
                EnvironmentMutation::Set { key, value } => {
                    let existing = candidate
                        .environment(environment_id)
                        .and_then(|environment| {
                            environment
                                .variables
                                .iter()
                                .find(|variable| variable.key == *key)
                                .map(|variable| {
                                    (variable.id.clone(), variable.enabled, variable.secret)
                                })
                        });
                    if let Some((id, enabled, secret)) = existing {
                        candidate.update_environment_variable(
                            environment_id,
                            &id,
                            key.clone(),
                            value.clone(),
                            enabled,
                            secret,
                        )
                    } else if let Some((original_id, enabled, secret)) =
                        original_metadata.get(key).cloned()
                    {
                        candidate
                            .add_environment_variable(
                                environment_id,
                                key.clone(),
                                value.clone(),
                                enabled,
                                secret,
                            )
                            .map(|temporary_id| {
                                if let Some(variable) = candidate
                                    .environments
                                    .iter_mut()
                                    .find(|environment| environment.id == environment_id)
                                    .and_then(|environment| {
                                        environment
                                            .variables
                                            .iter_mut()
                                            .find(|variable| variable.id == temporary_id)
                                    })
                                {
                                    variable.id = original_id;
                                }
                            })
                    } else {
                        candidate
                            .add_environment_variable(
                                environment_id,
                                key.clone(),
                                value.clone(),
                                true,
                                false,
                            )
                            .map(|_| ())
                    }
                }
                EnvironmentMutation::Unset { key } => {
                    let variable_id =
                        candidate
                            .environment(environment_id)
                            .and_then(|environment| {
                                environment
                                    .variables
                                    .iter()
                                    .find(|variable| variable.key == *key)
                                    .map(|variable| variable.id.clone())
                            });
                    variable_id.map_or(Ok(()), |id| {
                        candidate
                            .remove_environment_variable(environment_id, &id)
                            .map(|_| ())
                    })
                }
            };
            if let Err(error) = result {
                let message = format!("Script environment update was not saved: {error}");
                self.workspace_warning = Some(message.clone());
                return Err(message);
            }
        }

        self.commit_workspace(candidate)
            .map_err(|error| format!("Script environment update was not saved: {error}"))?;
        self.refresh_variable_intelligence(cx);
        if self.selected_environment_id.as_deref() == Some(environment_id) {
            self.reload_environment_editor(window, cx);
        }
        Ok(())
    }

    fn reload_environment_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let environment = self
            .selected_environment_id
            .as_deref()
            .and_then(|id| self.workspace.environment(id))
            .cloned();
        let name = environment
            .as_ref()
            .map(|environment| environment.name.clone())
            .unwrap_or_default();
        self.environment_name
            .update(cx, |input, cx| input.set_value(name, window, cx));
        self.environment_variables = Self::environment_rows(environment.as_ref(), window, cx);
    }

    fn update_response_editor(
        &mut self,
        response: &ResponseData,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let language = response_language(response);
        let content = if is_probably_text(&response.body) {
            format_body(&response.body, self.pretty_body)
        } else {
            format!(
                "Binary response ({}). The post-response script receives a bounded Base64 view.",
                format_bytes(response.size_bytes())
            )
        };
        self.response_editor.update(cx, |editor, cx| {
            editor.set_language(language, cx);
            editor.set_value(content, window, cx);
        });
    }

    fn update_body_language(&mut self, cx: &mut Context<Self>) {
        let language = code_language_for_raw_body(self.raw_body_language);
        self.body
            .update(cx, |editor, cx| editor.set_language(language, cx));
    }

    fn format_raw_body(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let source = self.body.read(cx).value(cx).to_string();
        if source.trim().is_empty() {
            self.request_notice = Some("The raw body buffer is empty.".to_owned());
            cx.notify();
            return;
        }

        let formatted = format_raw_body_source(self.raw_body_language, &source);
        let formatted = match formatted {
            Ok(formatted) => formatted,
            Err(message) => {
                self.request_notice = Some(message);
                cx.notify();
                return;
            }
        };
        if formatted == source {
            self.request_notice = Some("The raw body is already formatted.".to_owned());
            cx.notify();
            return;
        }

        let input = self.body.read(cx).input_state();
        input.update(cx, |input, cx| {
            let cursor = input.cursor_position();
            let full_range = 0..source.encode_utf16().count();
            EntityInputHandler::replace_text_in_range(
                input,
                Some(full_range),
                &formatted,
                window,
                cx,
            );
            input.set_cursor_position(cursor, window, cx);
        });
        self.request_notice = Some("Formatted raw JSON body.".to_owned());
        cx.notify();
    }

    fn select_body_mode(&mut self, mode: BodyMode, window: &mut Window, cx: &mut Context<Self>) {
        self.body_mode = mode;
        if matches!(mode, BodyMode::FormUrlEncoded | BodyMode::MultipartFormData)
            && self.body_fields.is_empty()
        {
            self.push_body_field_row("", "", true, BodyFieldKind::Text, window, cx);
        }
        if mode == BodyMode::Raw {
            self.update_body_language(cx);
        }
        self.refresh_request_dirty_part(RequestDirtyPart::BodyMode, cx);
        cx.notify();
    }

    fn select_raw_body_language(&mut self, language: RawBodyLanguage, cx: &mut Context<Self>) {
        self.raw_body_language = language;
        self.update_body_language(cx);
        self.refresh_request_dirty_part(RequestDirtyPart::RawBodyLanguage, cx);
        cx.notify();
    }

    fn choose_body_file(&mut self, row_id: usize, window: &mut Window, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose file".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = receiver.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| {
                if let Some(row) = this.body_fields.iter().find(|row| row.id == row_id) {
                    row.value.update(cx, |state, cx| {
                        state.set_value(path.display().to_string(), window, cx);
                    });
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn confirm_delete(&mut self, target: PendingDelete, cx: &mut Context<Self>) -> bool {
        if self.pending_delete.as_ref() == Some(&target) {
            self.pending_delete = None;
            true
        } else {
            self.pending_delete = Some(target);
            cx.notify();
            false
        }
    }

    fn clear_history(&mut self, cx: &mut Context<Self>) {
        if !self.history_writable {
            self.history_warning = Some(
                "History was not cleared because the database is read-only for this session."
                    .to_owned(),
            );
            cx.notify();
            return;
        }
        if !self.confirm_delete(PendingDelete::History, cx) {
            return;
        }

        let mut candidate = self.history.clone();
        candidate.clear();
        match self.database_store.save_history(&candidate) {
            Ok(()) => {
                self.history = candidate;
                self.history_warning = None;
            }
            Err(error) => {
                self.history_warning = Some(format!("History could not be cleared: {error}"));
            }
        }
        cx.notify();
    }

    fn load_history(
        &mut self,
        history_id: String,
        request: RequestDraft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.load_template(
            RequestTemplate::new(request),
            None,
            None,
            format!("history:{history_id}"),
            window,
            cx,
        );
    }

    fn load_template(
        &mut self,
        template: RequestTemplate,
        collection_id: Option<String>,
        saved_request_id: Option<String>,
        load_key: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.sending {
            return;
        }
        if self.request_is_dirty()
            && self.pending_request_load_key.as_deref() != Some(load_key.as_str())
        {
            self.pending_request_load_key = Some(load_key);
            self.request_notice = Some(
                "Unsaved request changes were kept. Click the same saved request or history entry again to discard them."
                    .to_owned(),
            );
            self.request_error = None;
            cx.notify();
            return;
        }

        self.request_dirty.begin_hydration();
        let RequestTemplate { request, scripts } = template;
        self.body_mode = request.body_mode;
        self.raw_body_language = request.raw_body_language;
        self.method.update(cx, |state, cx| {
            state.set_value(request.method, window, cx);
        });
        self.url.update(cx, |state, cx| {
            state.set_value(request.url, window, cx);
        });
        self.body.update(cx, |state, cx| {
            state.set_value(request.body, window, cx);
        });
        self.pre_request_script.update(cx, |editor, cx| {
            editor.set_value(scripts.pre_request, window, cx);
        });
        self.post_response_script.update(cx, |editor, cx| {
            editor.set_value(scripts.post_response, window, cx);
        });

        self.headers.clear();
        for header in request.headers {
            let was_redacted = header.value == REDACTED_VALUE;
            self.push_header_row(
                header.name,
                if was_redacted {
                    String::new()
                } else {
                    header.value
                },
                header.enabled && !was_redacted,
                window,
                cx,
            );
        }
        if self.headers.is_empty() {
            self.push_header_row("", "", true, window, cx);
        }

        self.body_fields.clear();
        for field in request.body_fields {
            self.push_body_field_row(
                field.name,
                field.value,
                field.enabled,
                field.kind,
                window,
                cx,
            );
        }
        if matches!(
            self.body_mode,
            BodyMode::FormUrlEncoded | BodyMode::MultipartFormData
        ) && self.body_fields.is_empty()
        {
            self.push_body_field_row("", "", true, BodyFieldKind::Text, window, cx);
        }
        self.update_body_language(cx);

        self.response = None;
        self.request_error = None;
        self.script_diagnostic = None;
        self.pre_script_report = None;
        self.post_script_report = None;
        self.preview_error = None;
        self.copied = false;
        if let Some(collection_id) = collection_id
            && let Some(collection) = self.workspace.collection(&collection_id)
        {
            self.selected_collection_id = Some(collection_id.clone());
            self.expanded_collection_ids.insert(collection_id);
            self.collection_name.update(cx, |input, cx| {
                input.set_value(collection.name.clone(), window, cx)
            });
            self.sidebar_tab = SidebarTab::Collections;
        }
        self.active_saved_request_id = saved_request_id.clone();
        self.detached_request_dirty = false;
        let request_name = saved_request_id
            .as_deref()
            .and_then(|id| self.workspace.saved_request(id))
            .map(|(_, request)| request.name.clone())
            .unwrap_or_default();
        self.saved_request_name
            .update(cx, |input, cx| input.set_value(request_name, window, cx));
        self.refresh_variable_intelligence(cx);
        self.loaded_request_baseline = self.request_template(cx);
        self.request_dirty.end_hydration();
        self.pending_request_load_key = None;
        self.request_notice = None;
        self.hide_preview(cx);
        cx.notify();
    }

    fn create_collection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.workspace_writable {
            return;
        }
        let name = unique_name(
            "Collection",
            self.workspace
                .collections
                .iter()
                .map(|collection| collection.name.as_str()),
        );
        let mut candidate = self.workspace.clone();
        match candidate.create_collection(name) {
            Ok(id) => {
                if self.commit_workspace(candidate).is_ok() {
                    self.expanded_collection_ids.insert(id.clone());
                    self.select_collection(id.clone(), window, cx);
                    self.renaming_collection_id = Some(id);
                    self.collection_name.read(cx).focus_handle(cx).focus(window);
                } else {
                    cx.notify();
                }
            }
            Err(error) => {
                self.workspace_warning = Some(error.to_string());
                cx.notify();
            }
        }
    }

    fn select_collection(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_collection_id.as_deref() == Some(&id) {
            self.sidebar_tab = SidebarTab::Collections;
            cx.notify();
            return;
        }
        let Some(collection) = self.workspace.collection(&id) else {
            return;
        };
        self.selected_collection_id = Some(id);
        if self.active_saved_request_id.take().is_some() {
            self.detached_request_dirty = true;
        }
        self.collection_name.update(cx, |input, cx| {
            input.set_value(collection.name.clone(), window, cx)
        });
        self.saved_request_name
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.sidebar_tab = SidebarTab::Collections;
        cx.notify();
    }

    fn toggle_collection(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if !self.expanded_collection_ids.remove(&id) {
            self.expanded_collection_ids.insert(id.clone());
        }
        self.select_collection(id, window, cx);
    }

    fn begin_collection_rename(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.select_collection(id.clone(), window, cx);
        self.renaming_collection_id = Some(id);
        self.collection_name.read(cx).focus_handle(cx).focus(window);
        cx.notify();
    }

    fn finish_collection_rename(&mut self, cx: &mut Context<Self>) {
        self.rename_collection(cx);
        self.renaming_collection_id = None;
        cx.notify();
    }

    fn rename_collection(&mut self, cx: &mut Context<Self>) {
        if !self.workspace_writable {
            return;
        }
        let Some(id) = self.selected_collection_id.clone() else {
            return;
        };
        let name = self.collection_name.read(cx).value().to_string();
        let mut candidate = self.workspace.clone();
        match candidate.rename_collection(&id, name) {
            Ok(()) => {
                let _ = self.commit_workspace(candidate);
            }
            Err(error) => self.workspace_warning = Some(error.to_string()),
        }
        cx.notify();
    }

    fn open_collection_delete_dialog(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.workspace_writable {
            return;
        }
        let Some(collection) = self.workspace.collection(&id) else {
            return;
        };
        let collection_name = collection.name.clone();
        self.collection_delete_confirmation
            .update(cx, |input, cx| input.set_value("", window, cx));

        let this = cx.entity().downgrade();
        let confirmation_input = self.collection_delete_confirmation.clone();
        window.open_dialog(cx, move |dialog, _, cx| {
            let id_for_ok = id.clone();
            let name_for_ok = collection_name.clone();
            let this_for_ok = this.clone();
            let input_for_ok = confirmation_input.clone();
            let input_for_footer = confirmation_input.clone();
            let name_for_footer = collection_name.clone();

            dialog
                .title("Delete collection?")
                .w(px(480.))
                .confirm()
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("Delete collection")
                        .ok_variant(ButtonVariant::Danger),
                )
                .footer(move |ok, cancel, window, cx| {
                    let confirmed =
                        input_for_footer.read(cx).value().as_ref() == name_for_footer.as_str();
                    vec![
                        cancel(window, cx),
                        if confirmed {
                            ok(window, cx)
                        } else {
                            Button::new("delete-collection-disabled")
                                .label("Delete collection")
                                .danger()
                                .disabled(true)
                                .into_any_element()
                        },
                    ]
                })
                .on_ok(move |_, window, cx| {
                    if input_for_ok.read(cx).value().as_ref() != name_for_ok.as_str() {
                        return false;
                    }
                    let Some(this) = this_for_ok.upgrade() else {
                        return true;
                    };
                    this.update(cx, |this, cx| {
                        this.delete_collection(id_for_ok.clone(), window, cx);
                    });
                    true
                })
                .child(
                    v_flex()
                        .gap_3()
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!(
                                    "This permanently deletes “{}” and every request inside it.",
                                    collection_name
                                )),
                        )
                        .child(
                            v_flex()
                                .gap_2()
                                .child(
                                    div()
                                        .text_sm()
                                        .child("Type the collection name to confirm:"),
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .font_semibold()
                                        .text_color(cx.theme().danger)
                                        .child(collection_name.clone()),
                                )
                                .child(Input::new(&confirmation_input)),
                        ),
                )
        });
        self.collection_delete_confirmation
            .read(cx)
            .focus_handle(cx)
            .focus(window);
    }

    fn delete_collection(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.workspace_writable {
            return false;
        }
        let deleting_selected = self.selected_collection_id.as_deref() == Some(id.as_str());
        let deleting_active_request =
            self.active_saved_request_id
                .as_deref()
                .is_some_and(|request_id| {
                    self.workspace.collection(&id).is_some_and(|collection| {
                        collection
                            .requests
                            .iter()
                            .any(|request| request.id == request_id)
                    })
                });
        let mut candidate = self.workspace.clone();
        match candidate.remove_collection(&id) {
            Ok(_) => {
                if self.commit_workspace(candidate).is_ok() {
                    self.expanded_collection_ids.remove(&id);
                    if self.renaming_collection_id.as_deref() == Some(id.as_str()) {
                        self.renaming_collection_id = None;
                    }
                    if deleting_active_request {
                        self.active_saved_request_id = None;
                        self.detached_request_dirty = true;
                        self.saved_request_name
                            .update(cx, |input, cx| input.set_value("", window, cx));
                    }
                    if deleting_selected {
                        self.selected_collection_id = self
                            .workspace
                            .collections
                            .first()
                            .map(|collection| collection.id.clone());
                        let name = self
                            .selected_collection_id
                            .as_deref()
                            .and_then(|id| self.workspace.collection(id))
                            .map(|collection| collection.name.clone())
                            .unwrap_or_default();
                        self.collection_name
                            .update(cx, |input, cx| input.set_value(name, window, cx));
                    }
                    self.request_notice = Some("Collection deleted.".to_owned());
                    cx.notify();
                    return true;
                }
            }
            Err(error) => self.workspace_warning = Some(error.to_string()),
        }
        cx.notify();
        false
    }

    fn save_current_request(&mut self, save_as: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.workspace_writable {
            return;
        }
        let Some(collection_id) = self.selected_collection_id.clone() else {
            self.workspace_warning =
                Some("Create or select a collection before saving this request.".to_owned());
            cx.notify();
            return;
        };
        let definition = self.request_template(cx);
        let entered_name = self.saved_request_name.read(cx).value().trim().to_owned();
        let name = if entered_name.is_empty() {
            default_request_name(&definition.request)
        } else {
            entered_name
        };

        let update_id = (!save_as)
            .then(|| self.active_saved_request_id.clone())
            .flatten()
            .filter(|id| {
                self.workspace
                    .collection(&collection_id)
                    .is_some_and(|collection| {
                        collection.requests.iter().any(|request| request.id == *id)
                    })
            });
        let mut candidate = self.workspace.clone();
        let result = if let Some(id) = update_id {
            candidate
                .update_saved_request(&collection_id, &id, definition)
                .and_then(|()| candidate.rename_saved_request(&collection_id, &id, name))
                .map(|()| id)
        } else {
            candidate.create_saved_request(&collection_id, name, definition)
        };

        match result {
            Ok(id) => {
                if self.commit_workspace(candidate).is_ok() {
                    self.active_saved_request_id = Some(id.clone());
                    let name = self
                        .workspace
                        .saved_request(&id)
                        .map(|(_, request)| request.name.clone())
                        .unwrap_or_default();
                    self.saved_request_name
                        .update(cx, |input, cx| input.set_value(name, window, cx));
                    self.loaded_request_baseline = self.request_template(cx);
                    self.detached_request_dirty = false;
                    self.request_dirty.clear();
                    self.pending_request_load_key = None;
                    self.request_notice = None;
                }
            }
            Err(error) => self.workspace_warning = Some(error.to_string()),
        }
        cx.notify();
    }

    fn open_saved_request_rename_dialog(
        &mut self,
        collection_id: String,
        request_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.workspace_writable || self.sending {
            return;
        }
        let Some(request_name) = self
            .workspace
            .collection(&collection_id)
            .and_then(|collection| {
                collection
                    .requests
                    .iter()
                    .find(|request| request.id == request_id)
                    .map(|request| request.name.clone())
            })
        else {
            return;
        };
        let rename_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Request name")
                .default_value(request_name)
        });
        let this = cx.entity().downgrade();
        let input_for_dialog = rename_input.clone();
        window.open_dialog(cx, move |dialog, _, cx| {
            let this_for_ok = this.clone();
            let input_for_ok = input_for_dialog.clone();
            let collection_id_for_ok = collection_id.clone();
            let request_id_for_ok = request_id.clone();
            dialog
                .title("Rename request")
                .w(px(440.))
                .confirm()
                .button_props(DialogButtonProps::default().ok_text("Rename"))
                .on_ok(move |_, window, cx| {
                    let name = input_for_ok.read(cx).value().trim().to_owned();
                    if name.is_empty() {
                        return false;
                    }
                    let Some(this) = this_for_ok.upgrade() else {
                        return true;
                    };
                    this.update(cx, |this, cx| {
                        this.rename_saved_request(
                            collection_id_for_ok.clone(),
                            request_id_for_ok.clone(),
                            name,
                            window,
                            cx,
                        );
                    });
                    true
                })
                .child(
                    v_flex()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child("Enter a new display name for this saved request."),
                        )
                        .child(Input::new(&input_for_dialog)),
                )
        });
        rename_input.read(cx).focus_handle(cx).focus(window);
    }

    fn rename_saved_request(
        &mut self,
        collection_id: String,
        request_id: String,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.workspace_writable || self.sending {
            return false;
        }
        let mut candidate = self.workspace.clone();
        match candidate.rename_saved_request(&collection_id, &request_id, name.clone()) {
            Ok(()) => {
                if self.commit_workspace(candidate).is_ok() {
                    if self.active_saved_request_id.as_deref() == Some(request_id.as_str()) {
                        self.saved_request_name
                            .update(cx, |input, cx| input.set_value(name.clone(), window, cx));
                    }
                    self.request_notice = Some(format!("Renamed request to “{name}”."));
                    cx.notify();
                    return true;
                }
            }
            Err(error) => self.workspace_warning = Some(error.to_string()),
        }
        cx.notify();
        false
    }

    fn duplicate_saved_request(
        &mut self,
        collection_id: String,
        request_id: String,
        cx: &mut Context<Self>,
    ) {
        if !self.workspace_writable || self.sending {
            return;
        }
        let Some(collection) = self.workspace.collection(&collection_id) else {
            return;
        };
        let Some(source) = collection
            .requests
            .iter()
            .find(|request| request.id == request_id)
        else {
            return;
        };
        let base_name = format!("{} copy", source.name);
        let duplicate_name = unique_name(
            &base_name,
            collection
                .requests
                .iter()
                .map(|request| request.name.as_str()),
        );
        let mut candidate = self.workspace.clone();
        match candidate.duplicate_saved_request(&collection_id, &request_id, duplicate_name.clone())
        {
            Ok(_) => {
                if self.commit_workspace(candidate).is_ok() {
                    self.expanded_collection_ids.insert(collection_id);
                    self.request_notice =
                        Some(format!("Duplicated request as “{duplicate_name}”."));
                }
            }
            Err(error) => self.workspace_warning = Some(error.to_string()),
        }
        cx.notify();
    }

    fn copy_saved_request_link(
        &mut self,
        collection_id: String,
        request_id: String,
        cx: &mut Context<Self>,
    ) {
        let Some(url) = self
            .workspace
            .collection(&collection_id)
            .and_then(|collection| {
                collection
                    .requests
                    .iter()
                    .find(|request| request.id == request_id)
                    .map(|request| request.definition.request.url.clone())
            })
        else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(url));
        self.request_notice = Some("Copied request link.".to_owned());
        cx.notify();
    }

    fn open_saved_request_delete_dialog(
        &mut self,
        collection_id: String,
        request_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.workspace_writable || self.sending {
            return;
        }
        let Some(request_name) = self
            .workspace
            .collection(&collection_id)
            .and_then(|collection| {
                collection
                    .requests
                    .iter()
                    .find(|request| request.id == request_id)
                    .map(|request| request.name.clone())
            })
        else {
            return;
        };
        let this = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let this_for_ok = this.clone();
            let collection_id_for_ok = collection_id.clone();
            let request_id_for_ok = request_id.clone();
            dialog
                .title("Delete request?")
                .w(px(440.))
                .confirm()
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("Delete request")
                        .ok_variant(ButtonVariant::Danger),
                )
                .on_ok(move |_, window, cx| {
                    let Some(this) = this_for_ok.upgrade() else {
                        return true;
                    };
                    this.update(cx, |this, cx| {
                        this.delete_saved_request(
                            collection_id_for_ok.clone(),
                            request_id_for_ok.clone(),
                            window,
                            cx,
                        );
                    });
                    true
                })
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!(
                            "“{request_name}” will be permanently removed from this collection."
                        )),
                )
        });
    }

    fn delete_saved_request(
        &mut self,
        collection_id: String,
        request_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.workspace_writable || self.sending {
            return false;
        }
        if self.workspace.collection(&collection_id).is_none() {
            return false;
        }
        let mut candidate = self.workspace.clone();
        match candidate.remove_saved_request(&collection_id, &request_id) {
            Ok(_) => {
                if self.commit_workspace(candidate).is_ok() {
                    if self.active_saved_request_id.as_deref() == Some(request_id.as_str()) {
                        self.active_saved_request_id = None;
                        self.detached_request_dirty = true;
                        self.saved_request_name
                            .update(cx, |input, cx| input.set_value("", window, cx));
                    }
                    self.request_notice = Some("Request deleted.".to_owned());
                    cx.notify();
                    return true;
                }
            }
            Err(error) => self.workspace_warning = Some(error.to_string()),
        }
        cx.notify();
        false
    }

    fn create_environment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.workspace_writable {
            return;
        }
        if self.environment_editor_is_dirty(cx) {
            self.workspace_warning =
                Some("Save the current environment before creating another.".to_owned());
            cx.notify();
            return;
        }
        let name = unique_name(
            "Environment",
            self.workspace
                .environments
                .iter()
                .map(|environment| environment.name.as_str()),
        );
        let mut candidate = self.workspace.clone();
        match candidate.create_environment(name) {
            Ok(id) => {
                let result = candidate.set_active_environment(Some(&id));
                if result.is_ok() && self.commit_workspace(candidate).is_ok() {
                    self.selected_environment_id = Some(id);
                    self.reload_environment_editor(window, cx);
                    self.refresh_variable_intelligence(cx);
                    self.sidebar_tab = SidebarTab::Environments;
                } else if let Err(error) = result {
                    self.workspace_warning = Some(error.to_string());
                }
            }
            Err(error) => self.workspace_warning = Some(error.to_string()),
        }
        cx.notify();
    }

    fn select_environment(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.workspace.environment(&id).is_none() {
            return;
        }
        if self.selected_environment_id.as_deref() == Some(&id) {
            self.sidebar_tab = SidebarTab::Environments;
            cx.notify();
            return;
        }
        if self.environment_editor_is_dirty(cx) {
            self.workspace_warning =
                Some("Save the current environment before switching.".to_owned());
            self.sidebar_tab = SidebarTab::Environments;
            cx.notify();
            return;
        }
        self.selected_environment_id = Some(id);
        self.reload_environment_editor(window, cx);
        self.sidebar_tab = SidebarTab::Environments;
        cx.notify();
    }

    fn environment_editor_is_dirty(&self, cx: &App) -> bool {
        let Some(environment) = self
            .selected_environment_id
            .as_deref()
            .and_then(|id| self.workspace.environment(id))
        else {
            return false;
        };
        if !input_text_equals(&self.environment_name, environment.name.as_str(), cx) {
            return true;
        }
        if self.environment_variables.len() != environment.variables.len() {
            return true;
        }
        self.environment_variables
            .iter()
            .zip(&environment.variables)
            .any(|(row, variable)| {
                row.id != variable.id
                    || !input_text_equals(&row.key, variable.key.as_str(), cx)
                    || !input_text_equals(&row.value, variable.value.as_str(), cx)
                    || row.enabled != variable.enabled
                    || row.secret != variable.secret
            })
    }

    fn activate_environment(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        if !self.workspace_writable || self.sending {
            return;
        }
        let mut candidate = self.workspace.clone();
        match candidate.set_active_environment(id.as_deref()) {
            Ok(()) => {
                if self.commit_workspace(candidate).is_ok() {
                    self.refresh_variable_intelligence(cx);
                }
            }
            Err(error) => self.workspace_warning = Some(error.to_string()),
        }
        cx.notify();
    }

    fn save_environment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.workspace_writable {
            return;
        }
        let Some(environment_id) = self.selected_environment_id.clone() else {
            return;
        };
        let name = self.environment_name.read(cx).value().to_string();
        let rows = self
            .environment_variables
            .iter()
            .map(|row| {
                (
                    row.id.clone(),
                    row.key.read(cx).value().to_string(),
                    row.value.read(cx).value().to_string(),
                    row.enabled,
                    row.secret,
                )
            })
            .collect::<Vec<_>>();
        let mut candidate = self.workspace.clone();
        let result = (|| {
            candidate.rename_environment(&environment_id, name)?;
            let current = candidate
                .environment(&environment_id)
                .cloned()
                .ok_or_else(|| crate::core::WorkspaceMutationError::NotFound {
                    kind: "environment",
                    id: environment_id.clone(),
                })?;
            let mut replacement = Environment {
                id: current.id,
                name: candidate
                    .environment(&environment_id)
                    .expect("renamed environment must still exist")
                    .name
                    .clone(),
                variables: Vec::with_capacity(rows.len()),
            };
            for (id, key, value, enabled, secret) in &rows {
                replacement.add_variable(key.clone(), value.clone(), *enabled, *secret)?;
                if !id.starts_with("draft-variable-") {
                    replacement
                        .variables
                        .last_mut()
                        .expect("add_variable must append")
                        .id = id.clone();
                }
            }
            let environment = candidate
                .environments
                .iter_mut()
                .find(|environment| environment.id == environment_id)
                .ok_or_else(|| crate::core::WorkspaceMutationError::NotFound {
                    kind: "environment",
                    id: environment_id.clone(),
                })?;
            *environment = replacement;
            Ok::<_, crate::core::WorkspaceMutationError>(())
        })();

        match result {
            Ok(()) => {
                if self.commit_workspace(candidate).is_ok() {
                    self.reload_environment_editor(window, cx);
                    self.refresh_variable_intelligence(cx);
                }
            }
            Err(error) => self.workspace_warning = Some(error.to_string()),
        }
        cx.notify();
    }

    fn revert_environment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.reload_environment_editor(window, cx);
        self.workspace_warning = None;
        cx.notify();
    }

    fn open_environment_delete_dialog(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.workspace_writable || self.sending {
            return;
        }
        let Some(environment) = self.workspace.environment(&id) else {
            return;
        };
        let environment_name = environment.name.clone();
        let active = self.workspace.active_environment_id.as_deref() == Some(id.as_str());
        let this = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let this_for_ok = this.clone();
            let id_for_ok = id.clone();
            dialog
                .title("Delete environment?")
                .w(px(460.))
                .confirm()
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("Delete environment")
                        .ok_variant(ButtonVariant::Danger),
                )
                .on_ok(move |_, window, cx| {
                    let Some(this) = this_for_ok.upgrade() else {
                        return true;
                    };
                    this.update(cx, |this, cx| {
                        this.delete_environment(id_for_ok.clone(), window, cx);
                    });
                    true
                })
                .child(
                    v_flex()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!(
                                    "“{environment_name}” and all of its variables will be permanently removed."
                                )),
                        )
                        .when(active, |this| {
                            this.child(
                                div()
                                    .px_3()
                                    .py_2()
                                    .rounded_md()
                                    .bg(cx.theme().warning.opacity(0.1))
                                    .text_xs()
                                    .text_color(cx.theme().warning)
                                    .child(
                                        "This is the active environment. Requests will switch to no environment.",
                                    ),
                            )
                        }),
                )
        });
    }

    fn delete_environment(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if !self.workspace_writable {
            return;
        }
        let deleting_selected = self.selected_environment_id.as_deref() == Some(id.as_str());
        let mut candidate = self.workspace.clone();
        match candidate.remove_environment(&id) {
            Ok(_) => {
                if self.commit_workspace(candidate).is_ok() {
                    if deleting_selected
                        || self
                            .selected_environment_id
                            .as_deref()
                            .is_some_and(|id| self.workspace.environment(id).is_none())
                    {
                        self.selected_environment_id = self
                            .workspace
                            .environments
                            .first()
                            .map(|environment| environment.id.clone());
                        self.reload_environment_editor(window, cx);
                    }
                    self.refresh_variable_intelligence(cx);
                }
            }
            Err(error) => self.workspace_warning = Some(error.to_string()),
        }
        cx.notify();
    }

    fn select_response_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.response_tab = ResponseTab::from_index(index);
        self.preview_error = None;
        self.copied = false;

        if self.response_tab == ResponseTab::Preview {
            self.show_preview(window, cx);
        } else {
            self.hide_preview(cx);
        }
        cx.notify();
    }

    fn show_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(response) = &self.response else {
            self.hide_preview(cx);
            return;
        };
        if !can_preview(response.content_type.as_deref(), &response.body) {
            self.hide_preview(cx);
            return;
        }

        let html = response.body_text_lossy();
        let preview = self
            .preview
            .get_or_insert_with(|| cx.new(|cx| HtmlPreview::new(window, cx)))
            .clone();
        let result = preview.update(cx, |preview, cx| preview.load_html(&html, cx));
        match result {
            Ok(()) => self.preview_error = None,
            Err(error) => {
                self.preview_error = Some(error.to_string());
                self.hide_preview(cx);
            }
        }
    }

    fn hide_preview(&mut self, cx: &mut Context<Self>) {
        if let Some(preview) = self.preview.take() {
            preview.update(cx, |preview, cx| preview.hide(cx));
        }
    }

    fn copy_response(&mut self, cx: &mut Context<Self>) {
        if self.response_tab == ResponseTab::Scripts {
            self.copy_script_results(cx);
            return;
        }

        let Some(response) = &self.response else {
            return;
        };

        let value = match self.response_tab {
            ResponseTab::Headers => response
                .headers
                .iter()
                .map(|header| format!("{}: {}", header.name, header.value))
                .collect::<Vec<_>>()
                .join("\n"),
            ResponseTab::Body => self.response_editor.read(cx).value(cx).to_string(),
            ResponseTab::Preview => format_body(&response.body, self.pretty_body),
            ResponseTab::Scripts => unreachable!("script copying is handled without a response"),
        };
        cx.write_to_clipboard(ClipboardItem::new_string(value));
        self.copied = true;
        cx.notify();
    }

    fn copy_script_results(&mut self, cx: &mut Context<Self>) {
        let model = script_console_model(
            self.pre_script_report.as_ref(),
            self.post_script_report.as_ref(),
            self.script_diagnostic.as_ref(),
            self.request_error.as_deref(),
        );
        cx.write_to_clipboard(ClipboardItem::new_string(model.copy_all_text()));
        self.copied = true;
        cx.notify();
    }

    fn request_header_count(&self, cx: &App) -> usize {
        self.headers
            .iter()
            .filter(|row| row.enabled && !input_text_is_blank(&row.name, cx))
            .count()
    }

    fn render_environment_title_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let active_environment_full = self
            .workspace
            .active_environment()
            .map(|environment| environment.name.clone())
            .unwrap_or_else(|| "No environment".to_owned());
        let active_environment = compact_label(&active_environment_full, 30);
        let active_environment_id = self.workspace.active_environment_id.clone();
        let environments = self
            .workspace
            .environments
            .iter()
            .map(|environment| (environment.id.clone(), environment.name.clone()))
            .collect::<Vec<_>>();
        let selected_name = self
            .selected_environment_id
            .as_deref()
            .and_then(|id| self.workspace.environment(id))
            .map(|environment| environment.name.as_str())
            .unwrap_or("No environment selected");
        let can_switch_environment = self.workspace_writable && !self.sending;
        let this = cx.entity().downgrade();

        h_flex()
            .h(px(64.))
            .flex_shrink_0()
            .pl(px(92.))
            .pr_6()
            .border_b_1()
            .border_color(cx.theme().title_bar_border)
            .bg(cx.theme().title_bar)
            .justify_between()
            .child(
                h_flex()
                    .min_w_0()
                    .gap_6()
                    .child(
                        div()
                            .text_xl()
                            .font_semibold()
                            .text_color(primary_bright())
                            .child("API Tester"),
                    )
                    .child(
                        h_flex()
                            .h_full()
                            .items_center()
                            .border_b_2()
                            .border_color(cx.theme().primary)
                            .px_1()
                            .text_sm()
                            .font_semibold()
                            .child("Environments"),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .max_w(px(420.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(selected_name.to_owned()),
                    ),
            )
            .child(
                Button::new("environment-workspace-active")
                    .icon(IconName::Settings2)
                    .label(active_environment)
                    .large()
                    .h(px(38.))
                    .outline()
                    .rounded(px(20.))
                    .tooltip(format!("Active environment: {active_environment_full}"))
                    .dropdown_menu(move |menu, _, _| {
                        let no_environment_this = this.clone();
                        let menu = menu.min_w(px(220.)).item(
                            PopupMenuItem::new("No environment")
                                .checked(active_environment_id.is_none())
                                .disabled(!can_switch_environment)
                                .on_click(move |_, _, cx| {
                                    if let Some(this) = no_environment_this.upgrade() {
                                        this.update(cx, |this, cx| {
                                            this.activate_environment(None, cx);
                                        });
                                    }
                                }),
                        );
                        environments.iter().fold(menu, |menu, (id, name)| {
                            let environment_id = id.clone();
                            let checked = active_environment_id.as_deref() == Some(id.as_str());
                            let environment_this = this.clone();
                            menu.item(
                                PopupMenuItem::new(name.clone())
                                    .checked(checked)
                                    .disabled(!can_switch_environment)
                                    .on_click(move |_, _, cx| {
                                        if let Some(this) = environment_this.upgrade() {
                                            this.update(cx, |this, cx| {
                                                this.activate_environment(
                                                    Some(environment_id.clone()),
                                                    cx,
                                                );
                                            });
                                        }
                                    }),
                            )
                        })
                    }),
            )
            .into_any_element()
    }

    fn render_title_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.sidebar_tab == SidebarTab::Environments {
            return self.render_environment_title_bar(cx);
        }

        let active_environment_full = self
            .workspace
            .active_environment()
            .map(|environment| environment.name.clone())
            .unwrap_or_else(|| "No environment".to_owned());
        let active_environment = compact_label(&active_environment_full, 30);
        let active_environment_id = self.workspace.active_environment_id.clone();
        let environments = self
            .workspace
            .environments
            .iter()
            .map(|environment| (environment.id.clone(), environment.name.clone()))
            .collect::<Vec<_>>();
        let this = cx.entity().downgrade();
        let request_name = self
            .active_saved_request_id
            .as_deref()
            .and_then(|id| self.workspace.saved_request(id))
            .map(|(_, request)| request.name.as_str())
            .unwrap_or("Unsaved request");
        let collection_name = self
            .selected_collection_id
            .as_deref()
            .and_then(|id| self.workspace.collection(id))
            .map(|collection| collection.name.as_str())
            .unwrap_or("No collection");
        let can_save =
            !self.sending && self.workspace_writable && self.selected_collection_id.is_some();
        let can_switch_environment = self.workspace_writable && !self.sending;
        let dirty = self.request_is_dirty();

        h_flex()
            .h(px(64.))
            .flex_shrink_0()
            .pl(px(92.))
            .pr_6()
            .border_b_1()
            .border_color(cx.theme().title_bar_border)
            .bg(cx.theme().title_bar)
            .justify_between()
            .child(
                h_flex()
                    .min_w_0()
                    .gap_6()
                    .child(
                        div()
                            .text_xl()
                            .font_semibold()
                            .text_color(primary_bright())
                            .child("API Tester"),
                    )
                    .child(
                        h_flex()
                            .h_full()
                            .items_center()
                            .border_b_2()
                            .border_color(cx.theme().primary)
                            .px_1()
                            .text_sm()
                            .font_semibold()
                            .child("Workspace"),
                    )
                    .child(
                        h_flex()
                            .min_w_0()
                            .gap_2()
                            .child(
                                div()
                                    .min_w_0()
                                    .max_w(px(360.))
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!("{collection_name} / {request_name}")),
                            )
                            .when(dirty, |this| {
                                this.child(
                                    div()
                                        .px_2()
                                        .py_1()
                                        .rounded_md()
                                        .bg(cx.theme().warning.opacity(0.12))
                                        .text_xs()
                                        .font_semibold()
                                        .text_color(cx.theme().warning)
                                        .child("Modified"),
                                )
                            }),
                    ),
            )
            .child(
                h_flex()
                    .flex_shrink_0()
                    .gap_3()
                    .child(
                        div()
                            .w(px(190.))
                            .child(Input::new(&self.saved_request_name).small()),
                    )
                    .child(
                        Button::new("active-environment")
                            .icon(IconName::Settings2)
                            .label(active_environment)
                            .large()
                            .h(px(38.))
                            .outline()
                            .rounded(px(20.))
                            .tooltip(active_environment_full)
                            .dropdown_menu(move |menu, _, _| {
                                let no_environment_this = this.clone();
                                let menu = menu.min_w(px(220.)).item(
                                    PopupMenuItem::new("No environment")
                                        .checked(active_environment_id.is_none())
                                        .disabled(!can_switch_environment)
                                        .on_click(move |_, _, cx| {
                                            if let Some(this) = no_environment_this.upgrade() {
                                                this.update(cx, |this, cx| {
                                                    this.activate_environment(None, cx);
                                                });
                                            }
                                        }),
                                );
                                let menu = environments.iter().fold(menu, |menu, (id, name)| {
                                    let environment_id = id.clone();
                                    let checked =
                                        active_environment_id.as_deref() == Some(id.as_str());
                                    let environment_this = this.clone();
                                    menu.item(
                                        PopupMenuItem::new(name.clone())
                                            .checked(checked)
                                            .disabled(!can_switch_environment)
                                            .on_click(move |_, _, cx| {
                                                if let Some(this) = environment_this.upgrade() {
                                                    this.update(cx, |this, cx| {
                                                        this.activate_environment(
                                                            Some(environment_id.clone()),
                                                            cx,
                                                        );
                                                    });
                                                }
                                            }),
                                    )
                                });
                                let manage_this = this.clone();
                                menu.separator().item(
                                    PopupMenuItem::new("Manage environments…").on_click(
                                        move |_, _, cx| {
                                            if let Some(this) = manage_this.upgrade() {
                                                this.update(cx, |this, cx| {
                                                    this.sidebar_tab = SidebarTab::Environments;
                                                    cx.notify();
                                                });
                                            }
                                        },
                                    ),
                                )
                            }),
                    )
                    .child(
                        Button::new("title-save-request")
                            .label(if self.active_saved_request_id.is_some() {
                                "Update"
                            } else {
                                "Save"
                            })
                            .large()
                            .h(px(38.))
                            .outline()
                            .rounded(px(20.))
                            .disabled(!can_save)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.save_current_request(false, window, cx);
                            })),
                    )
                    .when(self.active_saved_request_id.is_some(), |this| {
                        this.child(
                            Button::new("title-save-request-copy")
                                .label("Save as")
                                .large()
                                .h(px(38.))
                                .ghost()
                                .rounded(px(20.))
                                .disabled(!can_save)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.save_current_request(true, window, cx);
                                })),
                        )
                    }),
            )
            .into_any_element()
    }

    fn render_navigation_rail(&self, cx: &mut Context<Self>) -> AnyElement {
        let hud_visible = self.debug_overlay.read(cx).is_visible();
        let compact = self.navigation_compact;
        let rail_width = if compact { px(56.) } else { px(116.) };
        let item_width = if compact { px(44.) } else { px(100.) };
        let item_height = if compact { px(44.) } else { px(56.) };

        v_flex()
            .w(rail_width)
            .h_full()
            .flex_shrink_0()
            .items_center()
            .gap_2()
            .py_3()
            .border_r_1()
            .border_color(cx.theme().sidebar_border)
            .bg(surface_low())
            .child(
                v_flex()
                    .id("rail-collections")
                    .w(item_width)
                    .h(item_height)
                    .items_center()
                    .justify_center()
                    .gap_1()
                    .rounded_lg()
                    .cursor_pointer()
                    .text_color(cx.theme().muted_foreground)
                    .when(self.sidebar_tab == SidebarTab::Collections, |this| {
                        this.bg(cx.theme().sidebar_accent)
                            .text_color(cx.theme().foreground)
                    })
                    .hover(|style| style.bg(cx.theme().sidebar_accent))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.sidebar_tab = SidebarTab::Collections;
                        cx.notify();
                    }))
                    .when(compact, |this| {
                        this.tooltip(|window, cx| Tooltip::new("Collections").build(window, cx))
                    })
                    .child(gpui_component::Icon::new(IconName::FolderOpen).with_size(px(18.)))
                    .when(!compact, |this| {
                        this.child(
                            div()
                                .text_size(px(10.5))
                                .font_semibold()
                                .child("Collections"),
                        )
                    }),
            )
            .child(
                v_flex()
                    .id("rail-environments")
                    .w(item_width)
                    .h(item_height)
                    .items_center()
                    .justify_center()
                    .gap_1()
                    .rounded_lg()
                    .cursor_pointer()
                    .text_color(cx.theme().muted_foreground)
                    .when(self.sidebar_tab == SidebarTab::Environments, |this| {
                        this.bg(cx.theme().sidebar_accent)
                            .text_color(cx.theme().foreground)
                    })
                    .hover(|style| style.bg(cx.theme().sidebar_accent))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.sidebar_tab = SidebarTab::Environments;
                        cx.notify();
                    }))
                    .when(compact, |this| {
                        this.tooltip(|window, cx| Tooltip::new("Environments").build(window, cx))
                    })
                    .child(gpui_component::Icon::new(IconName::Settings2).with_size(px(18.)))
                    .when(!compact, |this| {
                        this.child(
                            div()
                                .text_size(px(10.5))
                                .font_semibold()
                                .child("Environments"),
                        )
                    }),
            )
            .child(
                v_flex()
                    .id("rail-history")
                    .w(item_width)
                    .h(item_height)
                    .items_center()
                    .justify_center()
                    .gap_1()
                    .rounded_lg()
                    .cursor_pointer()
                    .text_color(cx.theme().muted_foreground)
                    .when(self.sidebar_tab == SidebarTab::History, |this| {
                        this.bg(cx.theme().sidebar_accent)
                            .text_color(cx.theme().foreground)
                    })
                    .hover(|style| style.bg(cx.theme().sidebar_accent))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.sidebar_tab = SidebarTab::History;
                        cx.notify();
                    }))
                    .when(compact, |this| {
                        this.tooltip(|window, cx| Tooltip::new("History").build(window, cx))
                    })
                    .child(
                        gpui_component::Icon::new(IconName::GalleryVerticalEnd).with_size(px(18.)),
                    )
                    .when(!compact, |this| {
                        this.child(div().text_size(px(10.5)).font_semibold().child("History"))
                    }),
            )
            .child(div().flex_1())
            .child(
                v_flex()
                    .id("rail-hud")
                    .w(item_width)
                    .h(item_height)
                    .items_center()
                    .justify_center()
                    .gap_1()
                    .rounded_lg()
                    .cursor_pointer()
                    .text_color(cx.theme().muted_foreground)
                    .when(hud_visible, |this| {
                        this.bg(cx.theme().sidebar_accent)
                            .text_color(cx.theme().foreground)
                    })
                    .hover(|style| style.bg(cx.theme().sidebar_accent))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.debug_overlay
                            .update(cx, |overlay, cx| overlay.toggle(cx));
                        cx.notify();
                    }))
                    .when(compact, |this| {
                        this.tooltip(|window, cx| Tooltip::new("Metrics").build(window, cx))
                    })
                    .child(gpui_component::Icon::new(IconName::ChartPie).with_size(px(18.)))
                    .when(!compact, |this| {
                        this.child(div().text_size(px(10.5)).font_semibold().child("Metrics"))
                    }),
            )
            .child(
                h_flex()
                    .id("rail-compact-toggle")
                    .w(item_width)
                    .h(px(32.))
                    .justify_center()
                    .rounded_lg()
                    .cursor_pointer()
                    .text_color(cx.theme().muted_foreground)
                    .hover(|style| style.bg(cx.theme().sidebar_accent))
                    .tooltip(move |window, cx| {
                        Tooltip::new(if compact {
                            "Expand navigation"
                        } else {
                            "Collapse navigation"
                        })
                        .build(window, cx)
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.navigation_compact = !this.navigation_compact;
                        cx.notify();
                    }))
                    .child(
                        gpui_component::Icon::new(if compact {
                            IconName::ChevronRight
                        } else {
                            IconName::ChevronLeft
                        })
                        .with_size(px(16.)),
                    ),
            )
            .into_any_element()
    }

    fn render_history(&self, cx: &mut Context<Self>) -> AnyElement {
        let rows = self
            .history
            .entries()
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                let history_id = entry.id.clone();
                let status = entry.response.as_ref().map(|response| response.status);
                let status_text: SharedString = status
                    .map(|status| status.to_string())
                    .unwrap_or_else(|| "ERR".to_owned())
                    .into();
                let method: SharedString = entry.request.method.clone().into();
                let url: SharedString = compact_url(&entry.request.url).into();
                let time: SharedString = entry
                    .created_at
                    .with_timezone(&Local)
                    .format("%b %d · %H:%M")
                    .to_string()
                    .into();
                let color = method_color(&entry.request.method, cx);

                div()
                    .id(("history-entry", index))
                    .w_full()
                    .mb_1()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|style| style.bg(cx.theme().sidebar_accent))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let Some(request) = this
                            .history
                            .entries()
                            .iter()
                            .find(|entry| entry.id == history_id)
                            .map(|entry| entry.request.clone())
                        else {
                            return;
                        };
                        this.load_history(history_id.clone(), request, window, cx);
                    }))
                    .child(
                        h_flex()
                            .justify_between()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_semibold()
                                            .text_color(color)
                                            .child(method),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(status_text),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(time),
                            ),
                    )
                    .child(
                        div()
                            .mt_1()
                            .w_full()
                            .overflow_hidden()
                            .text_sm()
                            .whitespace_nowrap()
                            .child(url),
                    )
                    .into_any_element()
            })
            .collect::<Vec<_>>();

        v_flex()
            .size_full()
            .min_w_0()
            .h_full()
            .flex_shrink_0()
            .border_r_1()
            .border_color(cx.theme().sidebar_border)
            .bg(surface())
            .child(
                h_flex()
                    .h(px(56.))
                    .px_4()
                    .flex_shrink_0()
                    .justify_between()
                    .border_b_1()
                    .border_color(cx.theme().sidebar_border)
                    .child(
                        div()
                            .text_base()
                            .font_semibold()
                            .child(format!("History ({})", self.history.len())),
                    )
                    .child(
                        Button::new("clear-history")
                            .label(if self.pending_delete == Some(PendingDelete::History) {
                                "Confirm"
                            } else {
                                "Clear"
                            })
                            .xsmall()
                            .ghost()
                            .danger()
                            .disabled(self.history.is_empty() || !self.history_writable)
                            .tooltip("Click twice to permanently clear request history")
                            .on_click(cx.listener(|this, _, _, cx| this.clear_history(cx))),
                    ),
            )
            .child(
                div()
                    .id("history-scroll")
                    .flex_1()
                    .min_h_0()
                    .p_2()
                    .overflow_y_scroll()
                    .when(rows.is_empty(), |this| {
                        this.child(
                            v_flex()
                                .items_center()
                                .gap_1()
                                .px_4()
                                .py_8()
                                .text_center()
                                .text_color(cx.theme().muted_foreground)
                                .child(div().text_sm().child("No requests yet"))
                                .child(div().text_xs().child("Completed requests appear here.")),
                        )
                    })
                    .children(rows),
            )
            .when_some(self.history_warning.clone(), |this, warning| {
                this.child(
                    div()
                        .px_3()
                        .py_2()
                        .border_t_1()
                        .border_color(cx.theme().warning)
                        .text_xs()
                        .text_color(cx.theme().warning)
                        .child(warning),
                )
            })
            .into_any_element()
    }

    fn render_collections(&self, cx: &mut Context<Self>) -> AnyElement {
        let this = cx.entity().downgrade();
        let query = self
            .collection_search
            .read(cx)
            .value()
            .trim()
            .to_lowercase();
        let searching = !query.is_empty();
        let mut tree_rows = Vec::new();

        for (collection_index, collection) in self.workspace.collections.iter().enumerate() {
            let collection_matches = !searching || collection.name.to_lowercase().contains(&query);
            let matching_request_indexes = collection
                .requests
                .iter()
                .enumerate()
                .filter_map(|(index, request)| {
                    if !searching || collection_matches {
                        return Some(index);
                    }
                    let draft = &request.definition.request;
                    let matches = request.name.to_lowercase().contains(&query)
                        || draft.method.to_lowercase().contains(&query)
                        || draft.url.to_lowercase().contains(&query);
                    matches.then_some(index)
                })
                .collect::<Vec<_>>();

            if searching && !collection_matches && matching_request_indexes.is_empty() {
                continue;
            }

            let collection_id = collection.id.clone();
            let selected = self.selected_collection_id.as_deref() == Some(&collection.id);
            let expanded = searching
                || self
                    .expanded_collection_ids
                    .contains(collection.id.as_str());
            let renaming = self.renaming_collection_id.as_deref() == Some(&collection.id);
            let toggle_id = collection_id.clone();
            let select_id = collection_id.clone();
            let rename_id = collection_id.clone();
            let delete_id = collection_id.clone();
            let rename_this = this.clone();
            let delete_this = this.clone();

            tree_rows.push(
                h_flex()
                    .id(("collection-tree-row", collection_index))
                    .w_full()
                    .h(px(44.))
                    .px_1()
                    .gap_1()
                    .rounded_md()
                    .when(selected, |this| this.bg(cx.theme().sidebar_accent))
                    .hover(|style| style.bg(cx.theme().sidebar_accent.opacity(0.72)))
                    .child(
                        Button::new(("toggle-collection", collection_index))
                            .icon(if expanded {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            })
                            .xsmall()
                            .ghost()
                            .rounded_full()
                            .tooltip(if expanded {
                                "Collapse collection"
                            } else {
                                "Expand collection"
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.toggle_collection(toggle_id.clone(), window, cx);
                            })),
                    )
                    .child(
                        gpui_component::Icon::new(if expanded {
                            IconName::FolderOpen
                        } else {
                            IconName::FolderClosed
                        })
                        .small()
                        .text_color(if selected {
                            primary_bright()
                        } else {
                            cx.theme().muted_foreground
                        }),
                    )
                    .when(renaming, |this| {
                        this.child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(Input::new(&self.collection_name).small()),
                        )
                        .child(
                            Button::new(("finish-collection-rename", collection_index))
                                .icon(IconName::Check)
                                .xsmall()
                                .ghost()
                                .tooltip("Apply collection name")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.finish_collection_rename(cx);
                                })),
                        )
                    })
                    .when(!renaming, |this| {
                        this.child(
                            div()
                                .id(("select-collection", collection_index))
                                .flex_1()
                                .min_w_0()
                                .h_full()
                                .flex()
                                .items_center()
                                .cursor_pointer()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_sm()
                                .font_semibold()
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.select_collection(select_id.clone(), window, cx);
                                }))
                                .child(collection.name.clone()),
                        )
                        .child(
                            div()
                                .px_1()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(collection.requests.len().to_string()),
                        )
                        .child(
                            Button::new(("collection-actions", collection_index))
                                .icon(IconName::EllipsisVertical)
                                .xsmall()
                                .ghost()
                                .rounded_full()
                                .disabled(self.sending || !self.workspace_writable)
                                .dropdown_menu(move |menu, _, _| {
                                    let rename_id = rename_id.clone();
                                    let rename_this = rename_this.clone();
                                    let delete_id = delete_id.clone();
                                    let delete_this = delete_this.clone();
                                    menu.item(PopupMenuItem::new("Rename").on_click(
                                        move |_, window, cx| {
                                            if let Some(this) = rename_this.upgrade() {
                                                this.update(cx, |this, cx| {
                                                    this.begin_collection_rename(
                                                        rename_id.clone(),
                                                        window,
                                                        cx,
                                                    );
                                                });
                                            }
                                        },
                                    ))
                                    .item(
                                        PopupMenuItem::new("Delete…").on_click(
                                            move |_, window, cx| {
                                                let delete_this = delete_this.clone();
                                                let delete_id = delete_id.clone();
                                                window.defer(cx, move |window, cx| {
                                                    if let Some(this) = delete_this.upgrade() {
                                                        this.update(cx, |this, cx| {
                                                            this.open_collection_delete_dialog(
                                                                delete_id, window, cx,
                                                            );
                                                        });
                                                    }
                                                });
                                            },
                                        ),
                                    )
                                }),
                        )
                    })
                    .into_any_element(),
            );

            if !expanded {
                continue;
            }

            for request_index in matching_request_indexes {
                let request = &collection.requests[request_index];
                let request_id = request.id.clone();
                let load_id = request_id.clone();
                let load_collection_id = collection_id.clone();
                let selected = self.active_saved_request_id.as_deref() == Some(&request.id)
                    && self.selected_collection_id.as_deref() == Some(&collection.id);
                let method = request.definition.request.method.clone();
                let color = method_color(&method, cx);
                let load_key = format!("saved:{collection_id}:{load_id}");
                let row_element_id: SharedString =
                    format!("saved-request-row-{}-{}", collection.id, request.id).into();
                let action_group_id: SharedString =
                    format!("saved-request-actions-{}-{}", collection.id, request.id).into();
                let load_element_id: SharedString =
                    format!("load-saved-request-{}-{}", collection.id, request.id).into();
                let actions_element_id: SharedString =
                    format!("saved-request-menu-{}-{}", collection.id, request.id).into();
                let actions_this = this.clone();
                let actions_collection_id = collection_id.clone();
                let actions_request_id = request_id.clone();
                let can_mutate = !self.sending && self.workspace_writable;
                tree_rows.push(
                    h_flex()
                        .id(row_element_id)
                        .group(action_group_id.clone())
                        .w_full()
                        .h(px(42.))
                        .pl_9()
                        .pr_1()
                        .gap_1()
                        .rounded_md()
                        .when(selected, |this| this.bg(cx.theme().sidebar_accent))
                        .hover(|style| style.bg(cx.theme().sidebar_accent.opacity(0.72)))
                        .child(
                            h_flex()
                                .id(load_element_id)
                                .flex_1()
                                .min_w_0()
                                .h_full()
                                .gap_2()
                                .cursor_pointer()
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    let Some(definition) = this
                                        .workspace
                                        .saved_request(&load_id)
                                        .map(|(_, request)| request.definition.clone())
                                    else {
                                        return;
                                    };
                                    this.load_template(
                                        definition,
                                        Some(load_collection_id.clone()),
                                        Some(load_id.clone()),
                                        load_key.clone(),
                                        window,
                                        cx,
                                    );
                                }))
                                .child(
                                    div()
                                        .w(px(52.))
                                        .flex_shrink_0()
                                        .text_xs()
                                        .font_semibold()
                                        .text_color(color)
                                        .child(method.clone()),
                                )
                                .child(
                                    div()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_sm()
                                        .child(request.name.clone()),
                                ),
                        )
                        .child(
                            h_flex()
                                .w(px(28.))
                                .h_full()
                                .flex_shrink_0()
                                .justify_center()
                                .child(
                                    Button::new(actions_element_id)
                                        .icon(IconName::EllipsisVertical)
                                        .xsmall()
                                        .ghost()
                                        .rounded_full()
                                        .tooltip("Request actions")
                                        .invisible()
                                        .group_hover(action_group_id, |style| style.visible())
                                        .dropdown_menu(move |menu, _, _| {
                                            let rename_this = actions_this.clone();
                                            let rename_collection_id =
                                                actions_collection_id.clone();
                                            let rename_request_id = actions_request_id.clone();
                                            let duplicate_this = actions_this.clone();
                                            let duplicate_collection_id =
                                                actions_collection_id.clone();
                                            let duplicate_request_id =
                                                actions_request_id.clone();
                                            let copy_this = actions_this.clone();
                                            let copy_collection_id =
                                                actions_collection_id.clone();
                                            let copy_request_id = actions_request_id.clone();
                                            let delete_this = actions_this.clone();
                                            let delete_collection_id =
                                                actions_collection_id.clone();
                                            let delete_request_id = actions_request_id.clone();

                                            menu.item(
                                                PopupMenuItem::new("Rename")
                                                    .disabled(!can_mutate)
                                                    .on_click(move |_, window, cx| {
                                                        let rename_this = rename_this.clone();
                                                        let rename_collection_id =
                                                            rename_collection_id.clone();
                                                        let rename_request_id =
                                                            rename_request_id.clone();
                                                        window.defer(cx, move |window, cx| {
                                                            if let Some(this) =
                                                                rename_this.upgrade()
                                                            {
                                                                this.update(cx, |this, cx| {
                                                                    this.open_saved_request_rename_dialog(
                                                                        rename_collection_id,
                                                                        rename_request_id,
                                                                        window,
                                                                        cx,
                                                                    );
                                                                });
                                                            }
                                                        });
                                                    }),
                                            )
                                            .item(
                                                PopupMenuItem::new("Duplicate")
                                                    .disabled(!can_mutate)
                                                    .on_click(move |_, _, cx| {
                                                        if let Some(this) = duplicate_this.upgrade() {
                                                            this.update(cx, |this, cx| {
                                                                this.duplicate_saved_request(
                                                                    duplicate_collection_id.clone(),
                                                                    duplicate_request_id.clone(),
                                                                    cx,
                                                                );
                                                            });
                                                        }
                                                    }),
                                            )
                                            .item(PopupMenuItem::new("Copy link").on_click(
                                                move |_, _, cx| {
                                                    if let Some(this) = copy_this.upgrade() {
                                                        this.update(cx, |this, cx| {
                                                            this.copy_saved_request_link(
                                                                copy_collection_id.clone(),
                                                                copy_request_id.clone(),
                                                                cx,
                                                            );
                                                        });
                                                    }
                                                },
                                            ))
                                            .separator()
                                            .item(
                                                PopupMenuItem::new("Delete…")
                                                    .disabled(!can_mutate)
                                                    .on_click(move |_, window, cx| {
                                                        let delete_this = delete_this.clone();
                                                        let delete_collection_id =
                                                            delete_collection_id.clone();
                                                        let delete_request_id =
                                                            delete_request_id.clone();
                                                        window.defer(cx, move |window, cx| {
                                                            if let Some(this) =
                                                                delete_this.upgrade()
                                                            {
                                                                this.update(cx, |this, cx| {
                                                                    this.open_saved_request_delete_dialog(
                                                                        delete_collection_id,
                                                                        delete_request_id,
                                                                        window,
                                                                        cx,
                                                                    );
                                                                });
                                                            }
                                                        });
                                                    }),
                                            )
                                        }),
                                ),
                        )
                        .into_any_element(),
                );
            }
        }

        v_flex()
            .size_full()
            .min_w_0()
            .h_full()
            .flex_shrink_0()
            .border_r_1()
            .border_color(cx.theme().sidebar_border)
            .bg(surface())
            .child(
                h_flex()
                    .h(px(64.))
                    .px_3()
                    .gap_2()
                    .flex_shrink_0()
                    .border_b_1()
                    .border_color(cx.theme().sidebar_border)
                    .child(
                        Button::new("create-collection")
                            .icon(IconName::Plus)
                            .small()
                            .ghost()
                            .rounded_full()
                            .tooltip("New collection")
                            .disabled(self.sending || !self.workspace_writable)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.create_collection(window, cx);
                            })),
                    )
                    .child(
                        div().flex_1().min_w_0().child(
                            Input::new(&self.collection_search)
                                .prefix(IconName::Search)
                                .cleanable(true),
                        ),
                    ),
            )
            .child(
                v_flex()
                    .id("collections-scroll")
                    .flex_1()
                    .min_h_0()
                    .p_2()
                    .overflow_y_scroll()
                    .when(tree_rows.is_empty(), |this| {
                        this.child(
                            v_flex()
                                .items_center()
                                .gap_1()
                                .px_4()
                                .py_6()
                                .text_center()
                                .text_color(cx.theme().muted_foreground)
                                .child(div().text_sm().child(if searching {
                                    "No matching collections"
                                } else {
                                    "No collections"
                                }))
                                .child(div().text_xs().child(if searching {
                                    "Try another name, method, or URL."
                                } else {
                                    "Create one to save this request."
                                })),
                        )
                    })
                    .children(tree_rows),
            )
            .when_some(self.workspace_warning.clone(), |this, warning| {
                this.child(
                    div()
                        .px_3()
                        .py_2()
                        .border_t_1()
                        .border_color(cx.theme().warning)
                        .text_xs()
                        .text_color(cx.theme().warning)
                        .child(warning),
                )
            })
            .into_any_element()
    }

    fn render_environment_browser(&self, cx: &mut Context<Self>) -> AnyElement {
        let can_mutate = !self.sending && self.workspace_writable;
        let query = self
            .environment_search
            .read(cx)
            .value()
            .trim()
            .to_lowercase();
        let searching = !query.is_empty();
        let this = cx.entity().downgrade();
        let rows = self
            .workspace
            .environments
            .iter()
            .enumerate()
            .filter(|(_, environment)| {
                !searching || environment.name.to_lowercase().contains(&query)
            })
            .map(|(index, environment)| {
                let select_id = environment.id.clone();
                let action_environment_id = environment.id.clone();
                let action_environment_name = environment.name.clone();
                let selected = self.selected_environment_id.as_deref() == Some(&environment.id);
                let active =
                    self.workspace.active_environment_id.as_deref() == Some(&environment.id);
                let variable_count = environment.variables.len();
                let group_id: SharedString =
                    format!("environment-actions-{}", environment.id).into();
                let action_this = this.clone();

                h_flex()
                    .id(("environment-browser-row", index))
                    .group(group_id.clone())
                    .w_full()
                    .h(px(52.))
                    .px_2()
                    .gap_2()
                    .rounded_md()
                    .when(selected, |this| this.bg(cx.theme().sidebar_accent))
                    .hover(|style| style.bg(cx.theme().sidebar_accent.opacity(0.72)))
                    .child(
                        div()
                            .size_2()
                            .flex_shrink_0()
                            .rounded_full()
                            .border_1()
                            .border_color(if active {
                                cx.theme().primary
                            } else {
                                cx.theme().muted_foreground.opacity(0.5)
                            })
                            .when(active, |this| this.bg(cx.theme().primary)),
                    )
                    .child(
                        v_flex()
                            .id(("select-environment-browser-row", index))
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .justify_center()
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.select_environment(select_id.clone(), window, cx);
                            }))
                            .child(
                                div()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_sm()
                                    .font_semibold()
                                    .child(environment.name.clone()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(if active {
                                        format!("Active · {variable_count} variables")
                                    } else {
                                        format!("{variable_count} variables")
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .w(px(28.))
                            .h_full()
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                Button::new(("environment-browser-actions", index))
                                    .icon(IconName::EllipsisVertical)
                                    .xsmall()
                                    .ghost()
                                    .rounded_full()
                                    .tooltip("Environment actions")
                                    .invisible()
                                    .group_hover(group_id, |style| style.visible())
                                    .dropdown_menu(move |menu, _, _| {
                                        let activation_this = action_this.clone();
                                        let activation_id = action_environment_id.clone();
                                        let delete_this = action_this.clone();
                                        let delete_id = action_environment_id.clone();
                                        menu.item(
                                            PopupMenuItem::new(if active {
                                                "Stop using for requests"
                                            } else {
                                                "Use for requests"
                                            })
                                            .checked(active)
                                            .disabled(!can_mutate)
                                            .on_click(move |_, _, cx| {
                                                if let Some(this) = activation_this.upgrade() {
                                                    this.update(cx, |this, cx| {
                                                        this.activate_environment(
                                                            (!active)
                                                                .then(|| activation_id.clone()),
                                                            cx,
                                                        );
                                                    });
                                                }
                                            }),
                                        )
                                        .separator()
                                        .item(
                                            PopupMenuItem::new(format!(
                                                "Delete “{}”…",
                                                compact_label(&action_environment_name, 22)
                                            ))
                                            .disabled(!can_mutate)
                                            .on_click(move |_, window, cx| {
                                                let delete_this = delete_this.clone();
                                                let delete_id = delete_id.clone();
                                                window.defer(cx, move |window, cx| {
                                                    if let Some(this) = delete_this.upgrade() {
                                                        this.update(cx, |this, cx| {
                                                            this.open_environment_delete_dialog(
                                                                delete_id, window, cx,
                                                            );
                                                        });
                                                    }
                                                });
                                            }),
                                        )
                                    }),
                            ),
                    )
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        let no_matches = searching && rows.is_empty();
        let no_environment_active = self.workspace.active_environment_id.is_none();

        v_flex()
            .w(px(288.))
            .h_full()
            .flex_shrink_0()
            .border_r_1()
            .border_color(cx.theme().sidebar_border)
            .bg(surface_low())
            .child(
                h_flex()
                    .h(px(56.))
                    .px_4()
                    .flex_shrink_0()
                    .justify_between()
                    .border_b_1()
                    .border_color(cx.theme().sidebar_border)
                    .child(div().text_base().font_semibold().child("Environments"))
                    .child(
                        Button::new("create-environment")
                            .icon(IconName::Plus)
                            .small()
                            .ghost()
                            .rounded_full()
                            .tooltip("New environment")
                            .disabled(!can_mutate)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.create_environment(window, cx);
                            })),
                    ),
            )
            .child(
                h_flex()
                    .h(px(56.))
                    .px_3()
                    .flex_shrink_0()
                    .border_b_1()
                    .border_color(cx.theme().sidebar_border)
                    .child(
                        div().flex_1().min_w_0().child(
                            Input::new(&self.environment_search)
                                .prefix(IconName::Search)
                                .cleanable(true),
                        ),
                    ),
            )
            .child(
                v_flex()
                    .id("environments-browser-scroll")
                    .flex_1()
                    .min_h_0()
                    .gap_1()
                    .p_2()
                    .overflow_y_scroll()
                    .child(
                        h_flex()
                            .id("no-environment-row")
                            .w_full()
                            .h(px(52.))
                            .px_3()
                            .gap_3()
                            .rounded_md()
                            .cursor_pointer()
                            .when(no_environment_active, |this| {
                                this.bg(cx.theme().sidebar_accent)
                            })
                            .hover(|style| style.bg(cx.theme().sidebar_accent.opacity(0.72)))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.activate_environment(None, cx);
                            }))
                            .child(
                                div()
                                    .size_2()
                                    .flex_shrink_0()
                                    .rounded_full()
                                    .border_1()
                                    .border_color(if no_environment_active {
                                        cx.theme().primary
                                    } else {
                                        cx.theme().muted_foreground.opacity(0.5)
                                    })
                                    .when(no_environment_active, |this| {
                                        this.bg(cx.theme().primary)
                                    }),
                            )
                            .child(
                                v_flex()
                                    .min_w_0()
                                    .child(div().text_sm().font_semibold().child("No environment"))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child("Send requests without variables"),
                                    ),
                            ),
                    )
                    .children(rows)
                    .when(no_matches, |this| {
                        this.child(
                            v_flex()
                                .items_center()
                                .gap_1()
                                .px_4()
                                .py_6()
                                .text_center()
                                .text_color(cx.theme().muted_foreground)
                                .child(div().text_sm().child("No matching environments"))
                                .child(div().text_xs().child("Try another environment name.")),
                        )
                    }),
            )
            .when_some(self.workspace_warning.clone(), |this, warning| {
                this.child(
                    div()
                        .px_3()
                        .py_2()
                        .border_t_1()
                        .border_color(cx.theme().warning)
                        .text_xs()
                        .text_color(cx.theme().warning)
                        .child(warning),
                )
            })
            .into_any_element()
    }

    fn render_environment_variable_grid(&self, cx: &mut Context<Self>) -> AnyElement {
        let can_mutate = !self.sending && self.workspace_writable;
        let this = cx.entity().downgrade();
        let rows = self
            .environment_variables
            .iter()
            .enumerate()
            .map(|(index, row)| {
                let checkbox_id = row.id.clone();
                let secret_id = row.id.clone();
                let duplicate_id = row.id.clone();
                let remove_id = row.id.clone();
                let action_this = this.clone();
                let group_id: SharedString =
                    format!("environment-variable-actions-{}", row.id).into();

                h_flex()
                    .id(("environment-variable-grid-row", index))
                    .group(group_id.clone())
                    .w_full()
                    .h(px(46.))
                    .flex_shrink_0()
                    .border_t_1()
                    .border_color(outline_variant())
                    .bg(surface())
                    .hover(|style| style.bg(surface_low()))
                    .when(!row.enabled, |this| this.opacity(0.58))
                    .child(
                        div()
                            .w(px(44.))
                            .h_full()
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                Checkbox::new(("environment-variable-enabled", index))
                                    .checked(row.enabled)
                                    .small()
                                    .disabled(!can_mutate)
                                    .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                                        if let Some(row) = this
                                            .environment_variables
                                            .iter_mut()
                                            .find(|row| row.id == checkbox_id)
                                        {
                                            row.enabled = *checked;
                                            cx.notify();
                                        }
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .border_l_1()
                            .border_color(outline_variant())
                            .child(
                                Input::new(&row.key)
                                    .appearance(false)
                                    .small()
                                    .size_full()
                                    .px_3()
                                    .disabled(!can_mutate),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .border_l_1()
                            .border_color(outline_variant())
                            .child(
                                Input::new(&row.value)
                                    .appearance(false)
                                    .small()
                                    .size_full()
                                    .px_3()
                                    .disabled(!can_mutate),
                            ),
                    )
                    .child(
                        div()
                            .w(px(112.))
                            .h_full()
                            .flex_shrink_0()
                            .border_l_1()
                            .border_color(outline_variant())
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                Button::new(("secret-variable", index))
                                    .icon(if row.secret {
                                        IconName::EyeOff
                                    } else {
                                        IconName::Eye
                                    })
                                    .label(if row.secret { "Secret" } else { "Plain" })
                                    .xsmall()
                                    .ghost()
                                    .selected(row.secret)
                                    .disabled(!can_mutate)
                                    .tooltip(if row.secret {
                                        "Show as a plain variable"
                                    } else {
                                        "Mask this value in the UI and diagnostics"
                                    })
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        if let Some(row) = this
                                            .environment_variables
                                            .iter_mut()
                                            .find(|row| row.id == secret_id)
                                        {
                                            row.secret = !row.secret;
                                            let masked = row.secret;
                                            row.value.update(cx, |input, cx| {
                                                input.set_masked(masked, window, cx);
                                            });
                                            cx.notify();
                                        }
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .w(px(44.))
                            .h_full()
                            .flex_shrink_0()
                            .border_l_1()
                            .border_color(outline_variant())
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                Button::new(("environment-variable-actions", index))
                                    .icon(IconName::EllipsisVertical)
                                    .xsmall()
                                    .ghost()
                                    .rounded_full()
                                    .invisible()
                                    .group_hover(group_id, |style| style.visible())
                                    .tooltip("Variable actions")
                                    .dropdown_menu(move |menu, _, _| {
                                        let duplicate_this = action_this.clone();
                                        let duplicate_id = duplicate_id.clone();
                                        let remove_this = action_this.clone();
                                        let remove_id = remove_id.clone();
                                        menu.item(
                                            PopupMenuItem::new("Duplicate")
                                                .disabled(!can_mutate)
                                                .on_click(move |_, window, cx| {
                                                    if let Some(this) = duplicate_this.upgrade() {
                                                        this.update(cx, |this, cx| {
                                                            this.duplicate_environment_row(
                                                                &duplicate_id,
                                                                window,
                                                                cx,
                                                            );
                                                        });
                                                    }
                                                }),
                                        )
                                        .separator()
                                        .item(
                                            PopupMenuItem::new("Delete")
                                                .disabled(!can_mutate)
                                                .on_click(move |_, _, cx| {
                                                    if let Some(this) = remove_this.upgrade() {
                                                        this.update(cx, |this, cx| {
                                                            this.environment_variables
                                                                .retain(|row| row.id != remove_id);
                                                            cx.notify();
                                                        });
                                                    }
                                                }),
                                        )
                                    }),
                            ),
                    )
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        let empty = rows.is_empty();

        v_flex()
            .flex_1()
            .min_h_0()
            .rounded_lg()
            .border_1()
            .border_color(outline_variant())
            .overflow_hidden()
            .bg(surface())
            .child(
                h_flex()
                    .h(px(36.))
                    .flex_shrink_0()
                    .bg(surface_low())
                    .text_xs()
                    .font_semibold()
                    .text_color(cx.theme().muted_foreground)
                    .child(div().w(px(44.)).child(""))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .px_3()
                            .border_l_1()
                            .border_color(outline_variant())
                            .flex()
                            .items_center()
                            .child("KEY"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .px_3()
                            .border_l_1()
                            .border_color(outline_variant())
                            .flex()
                            .items_center()
                            .child("VALUE"),
                    )
                    .child(
                        div()
                            .w(px(112.))
                            .h_full()
                            .px_3()
                            .border_l_1()
                            .border_color(outline_variant())
                            .flex()
                            .items_center()
                            .child("VISIBILITY"),
                    )
                    .child(
                        div()
                            .w(px(44.))
                            .h_full()
                            .border_l_1()
                            .border_color(outline_variant()),
                    ),
            )
            .child(
                v_flex()
                    .id("environment-variable-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .when(empty, |this| {
                        this.child(
                            v_flex()
                                .h(px(92.))
                                .items_center()
                                .justify_center()
                                .gap_1()
                                .text_color(cx.theme().muted_foreground)
                                .child(div().text_sm().child("No variables yet"))
                                .child(
                                    div()
                                        .text_xs()
                                        .child("Add one below to start templating requests."),
                                ),
                        )
                    })
                    .children(rows)
                    .child(
                        h_flex()
                            .h(px(44.))
                            .flex_shrink_0()
                            .px_3()
                            .border_t_1()
                            .border_color(outline_variant())
                            .child(
                                Button::new("add-environment-variable")
                                    .icon(IconName::Plus)
                                    .label("Add variable")
                                    .small()
                                    .ghost()
                                    .disabled(!can_mutate)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.push_environment_row(window, cx);
                                        if let Some(input) = this
                                            .environment_variables
                                            .last()
                                            .map(|row| row.key.clone())
                                        {
                                            input.read(cx).focus_handle(cx).focus(window);
                                        }
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_environment_detail(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(environment) = self
            .selected_environment_id
            .as_deref()
            .and_then(|id| self.workspace.environment(id))
        else {
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_3()
                .p_8()
                .text_center()
                .bg(surface())
                .child(
                    gpui_component::Icon::new(IconName::Settings2)
                        .with_size(px(28.))
                        .text_color(cx.theme().muted_foreground),
                )
                .child(
                    div()
                        .text_lg()
                        .font_semibold()
                        .child("Create an environment"),
                )
                .child(
                    div()
                        .max_w(px(460.))
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(
                            "Keep reusable base URLs, tokens, and request values together, then reference them with {{variable}}.",
                        ),
                )
                .child(
                    Button::new("create-first-environment")
                        .icon(IconName::Plus)
                        .label("New environment")
                        .primary()
                        .disabled(self.sending || !self.workspace_writable)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.create_environment(window, cx);
                        })),
                )
                .into_any_element();
        };

        let environment_id = environment.id.clone();
        let environment_name = environment.name.clone();
        let editor_dirty = self.environment_editor_is_dirty(cx);
        let selected_is_active =
            self.workspace.active_environment_id.as_deref() == Some(environment_id.as_str());
        let active_editor_dirty = selected_is_active && editor_dirty;
        let variable_count = self.environment_variables.len();
        let can_mutate = !self.sending && self.workspace_writable;
        let activate_id = environment_id.clone();
        let delete_id = environment_id.clone();
        let delete_this = cx.entity().downgrade();

        v_flex()
            .size_full()
            .min_w_0()
            .bg(surface())
            .child(
                h_flex()
                    .h(px(76.))
                    .flex_shrink_0()
                    .px_6()
                    .gap_4()
                    .justify_between()
                    .border_b_1()
                    .border_color(outline_variant())
                    .child(
                        v_flex()
                            .w(px(440.))
                            .min_w_0()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("ENVIRONMENT NAME"),
                            )
                            .child(
                                Input::new(&self.environment_name)
                                    .large()
                                    .disabled(!can_mutate),
                            ),
                    )
                    .child(
                        h_flex()
                            .flex_shrink_0()
                            .gap_2()
                            .when(editor_dirty, |this| {
                                this.child(
                                    div()
                                        .px_2()
                                        .py_1()
                                        .rounded_md()
                                        .bg(cx.theme().warning.opacity(0.12))
                                        .text_xs()
                                        .font_semibold()
                                        .text_color(cx.theme().warning)
                                        .child(if active_editor_dirty {
                                            "Unsaved · blocks Send"
                                        } else {
                                            "Unsaved"
                                        }),
                                )
                            })
                            .child(
                                Button::new("activate-selected-environment")
                                    .label(if selected_is_active {
                                        "Active"
                                    } else {
                                        "Use for requests"
                                    })
                                    .small()
                                    .outline()
                                    .selected(selected_is_active)
                                    .disabled(!can_mutate)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.activate_environment(
                                            if selected_is_active {
                                                None
                                            } else {
                                                Some(activate_id.clone())
                                            },
                                            cx,
                                        );
                                    })),
                            )
                            .child(
                                Button::new("revert-environment")
                                    .label("Revert")
                                    .small()
                                    .ghost()
                                    .disabled(!can_mutate || !editor_dirty)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.revert_environment(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("save-environment")
                                    .label("Save")
                                    .small()
                                    .primary()
                                    .disabled(!can_mutate || !editor_dirty)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.save_environment(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("selected-environment-actions")
                                    .icon(IconName::EllipsisVertical)
                                    .small()
                                    .ghost()
                                    .rounded_full()
                                    .tooltip("Environment actions")
                                    .dropdown_menu(move |menu, _, _| {
                                        let delete_this = delete_this.clone();
                                        let delete_id = delete_id.clone();
                                        menu.item(
                                            PopupMenuItem::new(format!(
                                                "Delete “{}”…",
                                                compact_label(&environment_name, 24)
                                            ))
                                            .disabled(!can_mutate)
                                            .on_click(move |_, window, cx| {
                                                let delete_this = delete_this.clone();
                                                let delete_id = delete_id.clone();
                                                window.defer(cx, move |window, cx| {
                                                    if let Some(this) = delete_this.upgrade() {
                                                        this.update(cx, |this, cx| {
                                                            this.open_environment_delete_dialog(
                                                                delete_id, window, cx,
                                                            );
                                                        });
                                                    }
                                                });
                                            }),
                                        )
                                    }),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .h(px(42.))
                    .flex_shrink_0()
                    .px_6()
                    .gap_3()
                    .border_b_1()
                    .border_color(outline_variant())
                    .bg(surface_low())
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(if selected_is_active {
                                primary_bright()
                            } else {
                                cx.theme().muted_foreground
                            })
                            .child(if selected_is_active {
                                "ACTIVE FOR REQUESTS"
                            } else {
                                "NOT ACTIVE"
                            }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("{variable_count} variables")),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(
                                "Secret values are masked, but the local database is not encrypted.",
                            ),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .gap_3()
                    .p_6()
                    .child(
                        v_flex()
                            .gap_1()
                            .child(div().text_base().font_semibold().child("Variables"))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(
                                        "Reference enabled values in URLs, headers, bodies, and scripts.",
                                    ),
                            ),
                    )
                    .child(self.render_environment_variable_grid(cx)),
            )
            .into_any_element()
    }

    fn render_environment_workspace(&self, cx: &mut Context<Self>) -> AnyElement {
        h_flex()
            .size_full()
            .min_w_0()
            .bg(surface())
            .child(self.render_environment_browser(cx))
            .child(
                div()
                    .h_full()
                    .flex_1()
                    .min_w_0()
                    .child(self.render_environment_detail(cx)),
            )
            .into_any_element()
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        match self.sidebar_tab {
            SidebarTab::Collections => self.render_collections(cx),
            SidebarTab::Environments => self.render_environment_browser(cx),
            SidebarTab::History => self.render_history(cx),
        }
    }

    fn render_template_variable_popover(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let popover = self.template_variable_popover.clone()?;
        let blocker = self.template_variable_mutation_blocker(&popover, cx);
        let has_environment = popover.expected_environment_id.is_some();
        let is_create = matches!(popover.action, TemplateVariableAction::Create);
        let title = if is_create {
            "Create environment variable"
        } else {
            "Enable environment variable"
        };
        let action_label = if is_create { "Create" } else { "Enable" };
        let template = format!("{{{{{}}}}}", popover.name);
        let environment = popover
            .environment_name
            .clone()
            .unwrap_or_else(|| "No active environment".to_owned());
        let apply_this = cx.entity().downgrade();
        let close_this = apply_this.clone();
        let outside_this = apply_this.clone();

        let content = v_flex()
            .id("template-variable-popover")
            .w(px(340.))
            .gap_3()
            .p_4()
            .rounded_lg()
            .border_1()
            .border_color(outline_variant())
            .bg(surface_container())
            .shadow_lg()
            .on_mouse_down_out(move |_, _, cx| {
                if let Some(this) = outside_this.upgrade() {
                    this.update(cx, |this, cx| this.close_template_variable_popover(cx));
                }
            })
            .child(
                v_flex()
                    .gap_1()
                    .child(div().text_sm().font_semibold().child(title))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("{template} · {environment}")),
                    ),
            )
            .when(!has_environment, |this| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().warning)
                        .child("Choose an active environment before creating this variable."),
                )
                .child(
                    Button::new("template-open-environments")
                        .label("Open environments")
                        .small()
                        .outline()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.open_environments_from_template(cx);
                        })),
                )
            })
            .when(has_environment && is_create, |this| {
                this.child(
                    v_flex()
                        .gap_1()
                        .child(
                            div()
                                .text_xs()
                                .font_semibold()
                                .text_color(cx.theme().muted_foreground)
                                .child("VALUE"),
                        )
                        .child(
                            div()
                                .h(px(38.))
                                .w_full()
                                .rounded_md()
                                .border_1()
                                .border_color(outline_variant())
                                .bg(surface_lowest())
                                .child(
                                    Input::new(&popover.value)
                                        .appearance(false)
                                        .small()
                                        .size_full()
                                        .px_3()
                                        .disabled(blocker.is_some()),
                                ),
                        ),
                )
            })
            .when(has_environment && !is_create, |this| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(
                            "This variable exists but is disabled. Enable it without changing its value or secret status.",
                        ),
                )
            })
            .when_some(popover.error.clone().or_else(|| blocker.clone()), |this, message| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().red)
                        .child(message),
                )
            })
            .when(has_environment, |this| {
                this.child(
                    h_flex()
                        .w_full()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("template-variable-cancel")
                                .label("Cancel")
                                .small()
                                .ghost()
                                .on_click(move |_, _, cx| {
                                    if let Some(this) = close_this.upgrade() {
                                        this.update(cx, |this, cx| {
                                            this.close_template_variable_popover(cx)
                                        });
                                    }
                                }),
                        )
                        .child(
                            Button::new("template-variable-apply")
                                .label(action_label)
                                .small()
                                .primary()
                                .disabled(blocker.is_some())
                                .on_click(move |_, window, cx| {
                                    if let Some(this) = apply_this.upgrade() {
                                        this.update(cx, |this, cx| {
                                            this.apply_template_variable_action(window, cx)
                                        });
                                    }
                                }),
                        ),
                )
            });

        Some(
            deferred(
                anchored()
                    .position(popover.position)
                    .anchor(Corner::TopLeft)
                    .snap_to_window_with_margin(px(12.))
                    .child(content),
            )
            .with_priority(4)
            .into_any_element(),
        )
    }

    fn render_url_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let method = self.method.read(cx).value().trim().to_ascii_uppercase();
        let color = method_color(&method, cx);
        let selected_method = method.clone();
        let this = cx.entity().downgrade();
        let url_template_input = self.url.clone();
        let action = if self.sending {
            Button::new("cancel-request")
                .label("Cancel")
                .large()
                .h(px(44.))
                .rounded(px(12.))
                .danger()
                .on_click(cx.listener(|this, _, _, cx| this.cancel_request(cx)))
        } else {
            Button::new("send-request")
                .label("Send")
                .large()
                .h(px(44.))
                .rounded(px(12.))
                .primary()
                .on_click(
                    cx.listener(|this, _: &ClickEvent, window, cx| this.start_request(window, cx)),
                )
        };

        h_flex()
            .w_full()
            .h(px(44.))
            .gap_3()
            .child(
                h_flex()
                    .h_full()
                    .flex_1()
                    .min_w_0()
                    .rounded_lg()
                    .border_1()
                    .border_color(outline_variant())
                    .bg(surface_container())
                    .overflow_hidden()
                    .child(
                        h_flex()
                            .h_full()
                            .w(px(148.))
                            .flex_shrink_0()
                            .border_r_1()
                            .border_color(outline_variant())
                            .bg(color.opacity(0.08))
                            .child(
                                Popover::new("method-options")
                                    .appearance(false)
                                    .anchor(gpui::Corner::TopLeft)
                                    .track_focus(&self.method.read(cx).focus_handle(cx))
                                    .trigger(
                                        Input::new(&self.method)
                                            .appearance(false)
                                            .large()
                                            .w(px(148.))
                                            .font_semibold()
                                            .text_color(color)
                                            .suffix(
                                                gpui_component::Icon::new(IconName::ChevronDown)
                                                    .small()
                                                    .text_color(color),
                                            ),
                                    )
                                    .content(move |_, _, cx| {
                                        let popover = cx.entity();
                                        v_flex()
                                            .w(px(200.))
                                            .py_1()
                                            .rounded_lg()
                                            .border_1()
                                            .border_color(outline_variant())
                                            .bg(surface_container())
                                            .overflow_hidden()
                                            .children(STANDARD_HTTP_METHODS.iter().enumerate().map(
                                                |(index, method)| {
                                                    let method = (*method).to_owned();
                                                    let click_method = method.clone();
                                                    let this = this.clone();
                                                    let popover = popover.clone();
                                                    let selected = selected_method == method;
                                                    h_flex()
                                                        .id(("method-option", index))
                                                        .h(px(36.))
                                                        .w_full()
                                                        .px_3()
                                                        .gap_2()
                                                        .cursor_pointer()
                                                        .when(selected, |this| {
                                                            this.bg(cx.theme().sidebar_accent)
                                                        })
                                                        .hover(|style| style.bg(cx.theme().accent))
                                                        .child(
                                                            div().w(px(16.)).flex_shrink_0().when(
                                                                selected,
                                                                |this| {
                                                                    this.child(
                                                                        gpui_component::Icon::new(
                                                                            IconName::Check,
                                                                        )
                                                                        .xsmall(),
                                                                    )
                                                                },
                                                            ),
                                                        )
                                                        .child(
                                                            div()
                                                                .flex_1()
                                                                .font_semibold()
                                                                .text_color(method_color(
                                                                    &method, cx,
                                                                ))
                                                                .child(method),
                                                        )
                                                        .child(
                                                            div()
                                                                .text_xs()
                                                                .text_color(
                                                                    cx.theme().muted_foreground,
                                                                )
                                                                .child("HTTP"),
                                                        )
                                                        .on_click(move |_, window, cx| {
                                                            if let Some(this) = this.upgrade() {
                                                                this.update(cx, |this, cx| {
                                                                    this.method.update(
                                                                        cx,
                                                                        |state, cx| {
                                                                            state.set_value(
                                                                                click_method
                                                                                    .clone(),
                                                                                window,
                                                                                cx,
                                                                            );
                                                                            state.focus(window, cx);
                                                                        },
                                                                    );
                                                                    cx.notify();
                                                                });
                                                            }
                                                            popover.update(cx, |popover, cx| {
                                                                popover.dismiss(window, cx);
                                                            });
                                                        })
                                                },
                                            ))
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, event, window, cx| {
                                    this.open_template_variable_popover(
                                        url_template_input.clone(),
                                        event,
                                        window,
                                        cx,
                                    );
                                }),
                            )
                            .child(Input::new(&self.url).appearance(false).large()),
                    ),
            )
            .child(action)
            .into_any_element()
    }

    fn render_headers_editor(&self, cx: &mut Context<Self>) -> AnyElement {
        let this = cx.entity().downgrade();
        let rows = self
            .headers
            .iter()
            .map(|row| {
                let id = row.id;
                let action_this = this.clone();
                let group_id: SharedString = format!("header-row-actions-{id}").into();
                let name_template_input = row.name.clone();
                let value_template_input = row.value.clone();
                h_flex()
                    .id(("header-grid-row", id))
                    .group(group_id.clone())
                    .w_full()
                    .h(px(44.))
                    .flex_shrink_0()
                    .border_t_1()
                    .border_color(outline_variant())
                    .bg(surface())
                    .hover(|style| style.bg(surface_low()))
                    .when(!row.enabled, |this| this.opacity(0.55))
                    .child(
                        div()
                            .w(px(44.))
                            .h_full()
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                Checkbox::new(("header-enabled", id))
                                    .checked(row.enabled)
                                    .small()
                                    .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                                        if let Some(row) =
                                            this.headers.iter_mut().find(|row| row.id == id)
                                        {
                                            row.enabled = *checked;
                                            this.refresh_request_dirty_part(
                                                RequestDirtyPart::Headers,
                                                cx,
                                            );
                                            cx.notify();
                                        }
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .border_l_1()
                            .border_color(outline_variant())
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, event, window, cx| {
                                    this.open_template_variable_popover(
                                        name_template_input.clone(),
                                        event,
                                        window,
                                        cx,
                                    );
                                }),
                            )
                            .child(
                                Input::new(&row.name)
                                    .appearance(false)
                                    .small()
                                    .size_full()
                                    .px_3(),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .border_l_1()
                            .border_color(outline_variant())
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, event, window, cx| {
                                    this.open_template_variable_popover(
                                        value_template_input.clone(),
                                        event,
                                        window,
                                        cx,
                                    );
                                }),
                            )
                            .child(
                                Input::new(&row.value)
                                    .appearance(false)
                                    .small()
                                    .size_full()
                                    .px_3(),
                            ),
                    )
                    .child(
                        div()
                            .w(px(44.))
                            .h_full()
                            .flex_shrink_0()
                            .border_l_1()
                            .border_color(outline_variant())
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                Button::new(("header-actions", id))
                                    .icon(IconName::EllipsisVertical)
                                    .xsmall()
                                    .ghost()
                                    .rounded_full()
                                    .invisible()
                                    .group_hover(group_id, |style| style.visible())
                                    .tooltip("Header actions")
                                    .dropdown_menu(move |menu, _, _| {
                                        let duplicate_this = action_this.clone();
                                        let copy_this = action_this.clone();
                                        let remove_this = action_this.clone();
                                        menu.item(PopupMenuItem::new("Duplicate").on_click(
                                            move |_, window, cx| {
                                                if let Some(this) = duplicate_this.upgrade() {
                                                    this.update(cx, |this, cx| {
                                                        this.duplicate_header_row(id, window, cx);
                                                    });
                                                }
                                            },
                                        ))
                                        .item(PopupMenuItem::new("Copy header").on_click(
                                            move |_, _, cx| {
                                                if let Some(this) = copy_this.upgrade() {
                                                    this.update(cx, |this, cx| {
                                                        this.copy_header_row(id, cx);
                                                    });
                                                }
                                            },
                                        ))
                                        .separator()
                                        .item(
                                            PopupMenuItem::new("Delete").on_click(
                                                move |_, window, cx| {
                                                    if let Some(this) = remove_this.upgrade() {
                                                        this.update(cx, |this, cx| {
                                                            this.remove_header_row(id, window, cx);
                                                        });
                                                    }
                                                },
                                            ),
                                        )
                                    }),
                            ),
                    )
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        let enabled_count = self.request_header_count(cx);

        v_flex()
            .size_full()
            .min_h_0()
            .rounded_lg()
            .border_1()
            .border_color(outline_variant())
            .overflow_hidden()
            .bg(surface())
            .child(
                h_flex()
                    .h(px(42.))
                    .w_full()
                    .flex_shrink_0()
                    .px_3()
                    .justify_between()
                    .bg(surface_low())
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                div()
                                    .text_sm()
                                    .font_semibold()
                                    .child(format!("{enabled_count} enabled")),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("Sent in row order"),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .h(px(34.))
                    .w_full()
                    .flex_shrink_0()
                    .bg(surface_low())
                    .text_xs()
                    .font_semibold()
                    .text_color(cx.theme().muted_foreground)
                    .child(div().w(px(44.)).child(""))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .px_3()
                            .border_l_1()
                            .border_color(outline_variant())
                            .flex()
                            .items_center()
                            .child("KEY"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .px_3()
                            .border_l_1()
                            .border_color(outline_variant())
                            .flex()
                            .items_center()
                            .child("VALUE"),
                    )
                    .child(
                        div()
                            .w(px(44.))
                            .h_full()
                            .border_l_1()
                            .border_color(outline_variant()),
                    ),
            )
            .child(
                v_flex()
                    .id("header-rows")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(rows)
                    .child(
                        h_flex()
                            .h(px(42.))
                            .flex_shrink_0()
                            .px_3()
                            .border_t_1()
                            .border_color(outline_variant())
                            .child(
                                Button::new("add-header-row")
                                    .icon(IconName::Plus)
                                    .label("Add header")
                                    .small()
                                    .ghost()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.push_header_row("", "", true, window, cx);
                                        if let Some(input) =
                                            this.headers.last().map(|row| row.name.clone())
                                        {
                                            input.read(cx).focus_handle(cx).focus(window);
                                        }
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_body_mode_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let mode_buttons = BodyMode::all()
            .iter()
            .copied()
            .enumerate()
            .map(|(index, mode)| {
                Button::new(("body-mode", index))
                    .label(mode.label())
                    .small()
                    .ghost()
                    .rounded(px(18.))
                    .selected(self.body_mode == mode)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.select_body_mode(mode, window, cx);
                    }))
            })
            .collect::<Vec<_>>();

        let selected_language = self.raw_body_language;
        let this = cx.entity().downgrade();
        let language_selector = Button::new("raw-body-language")
            .label(selected_language.label())
            .dropdown_caret(true)
            .small()
            .outline()
            .rounded(px(18.))
            .dropdown_menu(move |menu, _, _| {
                RawBodyLanguage::all().iter().copied().fold(
                    menu.min_w(px(190.)).max_h(px(420.)).scrollable(true),
                    |menu, language| {
                        let this = this.clone();
                        menu.item(
                            PopupMenuItem::new(language.label())
                                .checked(language == selected_language)
                                .on_click(move |_, _, cx| {
                                    if let Some(this) = this.upgrade() {
                                        this.update(cx, |this, cx| {
                                            this.select_raw_body_language(language, cx);
                                        });
                                    }
                                }),
                        )
                    },
                )
            });

        v_flex()
            .w_full()
            .gap_1()
            .child(
                h_flex()
                    .w_full()
                    .flex_wrap()
                    .justify_between()
                    .gap_2()
                    .child(h_flex().flex_wrap().gap_1().children(mode_buttons))
                    .when(self.body_mode == BodyMode::Raw, |this| {
                        this.child(language_selector)
                    }),
            )
            .into_any_element()
    }

    fn render_body_fields_editor(&self, multipart: bool, cx: &mut Context<Self>) -> AnyElement {
        let this = cx.entity().downgrade();
        let rows = self
            .body_fields
            .iter()
            .map(|row| {
                let id = row.id;
                let is_file = row.kind == BodyFieldKind::File;
                let selected_kind = row.kind;
                let kind_this = this.clone();
                let action_this = this.clone();
                let group_id: SharedString = format!("body-field-row-actions-{id}").into();
                let name_template_input = row.name.clone();
                let value_template_input = row.value.clone();
                h_flex()
                    .id(("body-field-grid-row", id))
                    .group(group_id.clone())
                    .w_full()
                    .h(px(46.))
                    .flex_shrink_0()
                    .border_t_1()
                    .border_color(outline_variant())
                    .bg(surface())
                    .hover(|style| style.bg(surface_low()))
                    .when(!row.enabled, |this| this.opacity(0.55))
                    .child(
                        div()
                            .w(px(44.))
                            .h_full()
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                Checkbox::new(("body-field-enabled", id))
                                    .checked(row.enabled)
                                    .small()
                                    .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                                        if let Some(row) =
                                            this.body_fields.iter_mut().find(|row| row.id == id)
                                        {
                                            row.enabled = *checked;
                                            this.refresh_request_dirty_part(
                                                RequestDirtyPart::BodyFields,
                                                cx,
                                            );
                                            cx.notify();
                                        }
                                    })),
                            ),
                    )
                    .when(multipart, |this| {
                        this.child(
                            div()
                                .w(px(96.))
                                .h_full()
                                .flex_shrink_0()
                                .border_l_1()
                                .border_color(outline_variant())
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    Button::new(("body-field-kind", id))
                                        .label(selected_kind.label())
                                        .dropdown_caret(true)
                                        .xsmall()
                                        .ghost()
                                        .w(px(82.))
                                        .tooltip("Choose a text or file field")
                                        .dropdown_menu(move |menu, _, _| {
                                            BodyFieldKind::all().iter().copied().fold(
                                                menu.min_w(px(150.)),
                                                |menu, kind| {
                                                    let kind_this = kind_this.clone();
                                                    menu.item(
                                                        PopupMenuItem::new(kind.label())
                                                            .checked(kind == selected_kind)
                                                            .on_click(move |_, window, cx| {
                                                                if let Some(this) =
                                                                    kind_this.upgrade()
                                                                {
                                                                    this.update(cx, |this, cx| {
                                                                        this.set_body_field_kind(
                                                                            id, kind, window, cx,
                                                                        );
                                                                    });
                                                                }
                                                            }),
                                                    )
                                                },
                                            )
                                        }),
                                ),
                        )
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .border_l_1()
                            .border_color(outline_variant())
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, event, window, cx| {
                                    this.open_template_variable_popover(
                                        name_template_input.clone(),
                                        event,
                                        window,
                                        cx,
                                    );
                                }),
                            )
                            .child(
                                Input::new(&row.name)
                                    .appearance(false)
                                    .small()
                                    .size_full()
                                    .px_3(),
                            ),
                    )
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .border_l_1()
                            .border_color(outline_variant())
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, event, window, cx| {
                                    this.open_template_variable_popover(
                                        value_template_input.clone(),
                                        event,
                                        window,
                                        cx,
                                    );
                                }),
                            )
                            .child(
                                div().flex_1().min_w_0().h_full().child(
                                    Input::new(&row.value)
                                        .appearance(false)
                                        .small()
                                        .size_full()
                                        .px_3(),
                                ),
                            )
                            .when(multipart && is_file, |this| {
                                this.child(
                                    div().pr_2().child(
                                        Button::new(("choose-body-file", id))
                                            .icon(IconName::FolderOpen)
                                            .label("Choose")
                                            .xsmall()
                                            .outline()
                                            .tooltip("Choose a local file")
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.choose_body_file(id, window, cx);
                                            })),
                                    ),
                                )
                            }),
                    )
                    .child(
                        div()
                            .w(px(44.))
                            .h_full()
                            .flex_shrink_0()
                            .border_l_1()
                            .border_color(outline_variant())
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                Button::new(("body-field-actions", id))
                                    .icon(IconName::EllipsisVertical)
                                    .xsmall()
                                    .ghost()
                                    .rounded_full()
                                    .invisible()
                                    .group_hover(group_id, |style| style.visible())
                                    .tooltip("Field actions")
                                    .dropdown_menu(move |menu, _, _| {
                                        let duplicate_this = action_this.clone();
                                        let remove_this = action_this.clone();
                                        menu.item(PopupMenuItem::new("Duplicate").on_click(
                                            move |_, window, cx| {
                                                if let Some(this) = duplicate_this.upgrade() {
                                                    this.update(cx, |this, cx| {
                                                        this.duplicate_body_field_row(
                                                            id, window, cx,
                                                        );
                                                    });
                                                }
                                            },
                                        ))
                                        .separator()
                                        .item(
                                            PopupMenuItem::new("Delete").on_click(
                                                move |_, window, cx| {
                                                    if let Some(this) = remove_this.upgrade() {
                                                        this.update(cx, |this, cx| {
                                                            this.remove_body_field_row(
                                                                id, window, cx,
                                                            );
                                                        });
                                                    }
                                                },
                                            ),
                                        )
                                    }),
                            ),
                    )
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        let enabled_count = self
            .body_fields
            .iter()
            .filter(|row| row.enabled && !input_text_is_blank(&row.name, cx))
            .count();

        v_flex()
            .size_full()
            .min_h_0()
            .rounded_lg()
            .border_1()
            .border_color(outline_variant())
            .overflow_hidden()
            .bg(surface())
            .child(
                h_flex()
                    .h(px(42.))
                    .w_full()
                    .flex_shrink_0()
                    .px_3()
                    .bg(surface_low())
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                div()
                                    .text_sm()
                                    .font_semibold()
                                    .child(format!("{enabled_count} enabled")),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(if multipart {
                                        "Text fields and local file uploads"
                                    } else {
                                        "Encoded and sent in row order"
                                    }),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .h(px(34.))
                    .w_full()
                    .flex_shrink_0()
                    .bg(surface_low())
                    .text_xs()
                    .font_semibold()
                    .text_color(cx.theme().muted_foreground)
                    .child(div().w(px(44.)).child(""))
                    .when(multipart, |this| {
                        this.child(
                            div()
                                .w(px(96.))
                                .h_full()
                                .px_3()
                                .border_l_1()
                                .border_color(outline_variant())
                                .flex()
                                .items_center()
                                .child("TYPE"),
                        )
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .px_3()
                            .border_l_1()
                            .border_color(outline_variant())
                            .flex()
                            .items_center()
                            .child("KEY"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .px_3()
                            .border_l_1()
                            .border_color(outline_variant())
                            .flex()
                            .items_center()
                            .child("VALUE"),
                    )
                    .child(
                        div()
                            .w(px(44.))
                            .h_full()
                            .border_l_1()
                            .border_color(outline_variant()),
                    ),
            )
            .child(
                v_flex()
                    .id("body-field-rows")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(rows)
                    .child(
                        h_flex()
                            .h(px(42.))
                            .flex_shrink_0()
                            .px_3()
                            .border_t_1()
                            .border_color(outline_variant())
                            .child(
                                Button::new("add-body-field")
                                    .icon(IconName::Plus)
                                    .label("Add field")
                                    .small()
                                    .ghost()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.push_body_field_row(
                                            "",
                                            "",
                                            true,
                                            BodyFieldKind::Text,
                                            window,
                                            cx,
                                        );
                                        if let Some(input) =
                                            this.body_fields.last().map(|row| row.name.clone())
                                        {
                                            input.read(cx).focus_handle(cx).focus(window);
                                        }
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_body_editor(&self, cx: &mut Context<Self>) -> AnyElement {
        let content = match self.body_mode {
            BodyMode::None => v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_2()
                .text_color(cx.theme().muted_foreground)
                .child(
                    div()
                        .text_sm()
                        .font_semibold()
                        .child("This request has no body"),
                )
                .child(
                    div()
                        .text_xs()
                        .child("Choose Raw or a form mode above to add one."),
                )
                .into_any_element(),
            BodyMode::Raw => {
                let body_template_input = self.body.read(cx).input_state();
                div()
                    .size_full()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event, window, cx| {
                            this.open_template_variable_popover(
                                body_template_input.clone(),
                                event,
                                window,
                                cx,
                            );
                        }),
                    )
                    .child(self.body.clone())
                    .into_any_element()
            }
            BodyMode::FormUrlEncoded => self.render_body_fields_editor(false, cx),
            BodyMode::MultipartFormData => self.render_body_fields_editor(true, cx),
        };

        v_flex()
            .size_full()
            .min_h_0()
            .gap_3()
            .child(self.render_body_mode_toolbar(cx))
            .child(div().flex_1().min_h_0().child(content))
            .into_any_element()
    }

    fn render_request_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let header_count = self.request_header_count(cx);
        let status = if let Some(notice) = self.request_notice.clone() {
            Some(
                div()
                    .w_full()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .border_1()
                    .border_color(cx.theme().warning.opacity(0.65))
                    .bg(cx.theme().warning.opacity(0.08))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_xs()
                    .text_color(cx.theme().warning)
                    .child(notice)
                    .into_any_element(),
            )
        } else {
            self.execution_stage.map(|stage| {
                div()
                    .w_full()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(cx.theme().primary.opacity(0.08))
                    .text_xs()
                    .font_semibold()
                    .text_color(cx.theme().primary)
                    .child(stage.label())
                    .into_any_element()
            })
        };

        v_flex()
            .size_full()
            .min_h_0()
            .gap_3()
            .p_4()
            .bg(surface())
            .child(self.render_url_row(cx))
            .when_some(status, |this, status| this.child(status))
            .child(
                TabBar::new("request-tabs")
                    .underline()
                    .children([
                        format!("Headers ({header_count})"),
                        "Body".to_owned(),
                        "Pre-request".to_owned(),
                        "Post-response".to_owned(),
                    ])
                    .selected_index(self.request_tab.index())
                    .on_click(cx.listener(|this, index: &usize, _, cx| {
                        this.request_tab = RequestTab::from_index(*index);
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .when(self.request_tab == RequestTab::Headers, |this| {
                        this.child(self.render_headers_editor(cx))
                    })
                    .when(self.request_tab == RequestTab::Body, |this| {
                        this.child(self.render_body_editor(cx))
                    })
                    .when(self.request_tab == RequestTab::PreRequest, |this| {
                        this.child(self.pre_request_script.clone())
                    })
                    .when(self.request_tab == RequestTab::PostResponse, |this| {
                        this.child(self.post_response_script.clone())
                    }),
            )
            .into_any_element()
    }

    fn render_response_summary(
        &self,
        response: &ResponseData,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let status_color = status_color(response.status, cx);
        h_flex()
            .gap_3()
            .text_xs()
            .child(
                div()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(status_color.opacity(0.14))
                    .text_color(status_color)
                    .font_semibold()
                    .child(format!("{} {}", response.status, response.status_text)),
            )
            .child(
                div()
                    .text_color(cx.theme().muted_foreground)
                    .child(format_duration(response.duration)),
            )
            .child(
                div()
                    .text_color(cx.theme().muted_foreground)
                    .child(format_bytes(response.size_bytes())),
            )
            .child(
                div()
                    .text_color(cx.theme().muted_foreground)
                    .child(response.http_version.clone()),
            )
            .into_any_element()
    }

    fn render_response_body(&self, _cx: &mut Context<Self>) -> AnyElement {
        div()
            .size_full()
            .bg(surface_lowest())
            .child(self.response_editor.clone())
            .into_any_element()
    }

    fn render_script_results(&self, cx: &mut Context<Self>) -> AnyElement {
        let model = script_console_model(
            self.pre_script_report.as_ref(),
            self.post_script_report.as_ref(),
            self.script_diagnostic.as_ref(),
            self.request_error.as_deref(),
        );
        let row_count = model.row_count();
        let copy_label = if self.copied { "Copied" } else { "Copy all" };
        let generation = self.request_generation;
        let sections = model
            .sections
            .iter()
            .enumerate()
            .map(|(section_index, section)| {
                let section_key = section.key.clone();
                let duration = section
                    .duration
                    .map(format_script_duration)
                    .unwrap_or_default();
                let section_meta = if duration.is_empty() {
                    format!(
                        "{} {}",
                        section.rows.len(),
                        if section.rows.len() == 1 {
                            "entry"
                        } else {
                            "entries"
                        }
                    )
                } else {
                    format!(
                        "{duration} · {} {}",
                        section.rows.len(),
                        if section.rows.len() == 1 {
                            "entry"
                        } else {
                            "entries"
                        }
                    )
                };
                let rows = section.rows.iter().enumerate().map(|(row_index, row)| {
                    let group_id: SharedString = format!(
                        "script-console-row-group-{generation}-{section_key}-{section_index}-{row_index}"
                    )
                    .into();
                    let row_id: SharedString = format!(
                        "script-console-row-{generation}-{section_key}-{section_index}-{row_index}"
                    )
                    .into();
                    let copy_id: SharedString = format!(
                        "copy-script-console-row-{generation}-{section_key}-{section_index}-{row_index}"
                    )
                    .into();
                    let copy_hint_id: SharedString = format!(
                        "copy-script-console-row-hint-{generation}-{section_key}-{section_index}-{row_index}"
                    )
                    .into();
                    let tone_color = script_console_tone_color(row.tone, cx);
                    let tone_background = script_console_tone_background(row.tone, cx);

                    h_flex()
                        .id(row_id)
                        .group(group_id.clone())
                        .w_full()
                        .min_h(px(42.))
                        .items_start()
                        .px_3()
                        .py_2()
                        .gap_2()
                        .border_b_1()
                        .border_color(outline_variant())
                        .bg(tone_background)
                        .hover(|style| style.bg(surface_low()))
                        .child(
                            div()
                                .w(px(20.))
                                .h(px(22.))
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    gpui_component::Icon::new(script_console_tone_icon(row.tone))
                                        .with_size(px(14.))
                                        .text_color(tone_color),
                                ),
                        )
                        .child(
                            div()
                                .w(px(64.))
                                .h(px(22.))
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .text_xs()
                                .font_semibold()
                                .text_color(tone_color)
                                .child(row.label.clone()),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .font_family(cx.theme().mono_font_family.clone())
                                .text_size(cx.theme().mono_font_size)
                                .line_height(px(19.))
                                .child(
                                    div()
                                        .min_w_0()
                                        .whitespace_normal()
                                        .child(row.message.clone()),
                                )
                                .when_some(row.detail.clone(), |this, detail| {
                                    this.child(
                                        div()
                                            .mt_1()
                                            .pl_2()
                                            .border_l_1()
                                            .border_color(tone_color.opacity(0.5))
                                            .whitespace_normal()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(detail),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .id(copy_hint_id)
                                .w(px(28.))
                                .h(px(24.))
                                .flex_shrink_0()
                                .opacity(0.55)
                                .group_hover(group_id, |style| style.opacity(1.))
                                .tooltip(|window, cx| {
                                    Tooltip::new("Copy message").build(window, cx)
                                })
                                .child(
                                    Clipboard::new(copy_id).value(row.copy_value.clone()),
                                ),
                        )
                        .into_any_element()
                });

                v_flex()
                    .w_full()
                    .child(
                        h_flex()
                            .h(px(34.))
                            .flex_shrink_0()
                            .px_3()
                            .gap_2()
                            .border_b_1()
                            .border_color(outline_variant())
                            .bg(cx.theme().muted.opacity(0.34))
                            .child(
                                div()
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(cx.theme().foreground)
                                    .child(section.title.clone()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(section_meta),
                            ),
                    )
                    .children(rows)
                    .into_any_element()
            })
            .collect::<Vec<_>>();

        v_flex()
            .size_full()
            .rounded_md()
            .border_1()
            .border_color(outline_variant())
            .overflow_hidden()
            .bg(surface_lowest())
            .child(
                h_flex()
                    .h(px(40.))
                    .flex_shrink_0()
                    .px_3()
                    .gap_2()
                    .border_b_1()
                    .border_color(outline_variant())
                    .bg(surface_low())
                    .child(
                        gpui_component::Icon::new(IconName::SquareTerminal)
                            .with_size(px(15.))
                            .text_color(cx.theme().muted_foreground),
                    )
                    .child(div().text_sm().font_semibold().child("Script console"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!(
                                "{row_count} {}",
                                if row_count == 1 { "entry" } else { "entries" }
                            )),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new(("copy-all-script-output", generation))
                            .icon(if self.copied {
                                IconName::Check
                            } else {
                                IconName::Copy
                            })
                            .label(copy_label)
                            .small()
                            .ghost()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.copy_script_results(cx);
                            })),
                    ),
            )
            .child(
                div()
                    .id(("script-results-scroll", generation))
                    .flex_1()
                    .min_h_0()
                    .when(model.sections.is_empty(), |this| {
                        this.flex()
                            .items_center()
                            .justify_center()
                            .p_4()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child("No script has run yet.")
                    })
                    .children(sections)
                    .overflow_y_scrollbar(),
            )
            .into_any_element()
    }

    fn render_response_headers(
        &self,
        response: &ResponseData,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .id("response-headers-scroll")
            .size_full()
            .overflow_y_scroll()
            .children(response.headers.iter().enumerate().map(|(index, header)| {
                h_flex()
                    .px_3()
                    .py_2()
                    .gap_4()
                    .when(index > 0, |this| {
                        this.border_t_1().border_color(cx.theme().border)
                    })
                    .child(
                        div()
                            .w(px(220.))
                            .flex_shrink_0()
                            .text_sm()
                            .font_semibold()
                            .child(header.name.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .font_family(cx.theme().mono_font_family.clone())
                            .child(header.value.clone()),
                    )
            }))
            .into_any_element()
    }

    fn render_preview(&self, response: &ResponseData, cx: &mut Context<Self>) -> AnyElement {
        if !can_preview(response.content_type.as_deref(), &response.body) {
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_2()
                .text_color(cx.theme().muted_foreground)
                .child(div().text_sm().font_semibold().child("No HTML preview"))
                .child(div().text_xs().child(
                    "The response is not declared as HTML and has no HTML document markers.",
                ))
                .into_any_element();
        }

        v_flex()
            .size_full()
            .min_h_0()
            .child(
                h_flex()
                    .h_8()
                    .flex_shrink_0()
                    .px_3()
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().muted.opacity(0.45))
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("Captured response · JavaScript, network access, navigation, and downloads blocked"),
            )
            .when_some(self.preview_error.clone(), |this, error| {
                this.child(
                    div()
                        .px_3()
                        .py_2()
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .child(error),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .border_1()
                    .border_t_0()
                    .border_color(cx.theme().border)
                    .when_some(self.preview.clone(), |this, preview| this.child(preview)),
            )
            .into_any_element()
    }

    fn render_response_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(response) = &self.response else {
            let has_script_console = self.pre_script_report.is_some()
                || self.post_script_report.is_some()
                || self.script_diagnostic.is_some();
            let state_label = if self.sending {
                "Waiting for response…"
            } else if self.script_diagnostic.is_some() {
                "Script failed"
            } else if self.request_error.is_some() {
                "Request failed"
            } else {
                "No response yet"
            };
            return v_flex()
                .size_full()
                .min_h_0()
                .bg(surface())
                .child(
                    h_flex()
                        .h(px(56.))
                        .flex_shrink_0()
                        .px_4()
                        .gap_3()
                        .border_b_1()
                        .border_color(outline_variant())
                        .child(div().text_base().font_semibold().child("Response"))
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(state_label),
                        ),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_h_0()
                        .when(has_script_console, |this| {
                            this.p_4().child(self.render_script_results(cx))
                        })
                        .when(!has_script_console, |this| {
                            this.items_center()
                                .justify_center()
                                .gap_2()
                                .p_4()
                                .text_color(cx.theme().muted_foreground)
                                .when_some(self.request_error.clone(), |this, error| {
                                    this.child(
                                        div()
                                            .max_w(px(640.))
                                            .px_4()
                                            .py_3()
                                            .rounded_lg()
                                            .border_1()
                                            .border_color(cx.theme().danger)
                                            .bg(cx.theme().danger.opacity(0.08))
                                            .text_color(cx.theme().danger)
                                            .text_sm()
                                            .child(error),
                                    )
                                })
                                .when(self.request_error.is_none() && !self.sending, |this| {
                                    this.child(
                                        div()
                                            .text_base()
                                            .font_semibold()
                                            .text_color(cx.theme().foreground)
                                            .child("Ready to send"),
                                    )
                                    .child(div().text_sm().child(
                                        "Choose a method, enter a URL, then press Send or Return.",
                                    ))
                                })
                                .when(self.sending, |this| {
                                    this.child(
                                        div()
                                            .text_base()
                                            .font_semibold()
                                            .text_color(cx.theme().foreground)
                                            .child("Waiting for response…"),
                                    )
                                })
                        }),
                )
                .into_any_element();
        };

        v_flex()
            .size_full()
            .min_h_0()
            .bg(surface())
            .child(
                h_flex()
                    .h(px(56.))
                    .flex_shrink_0()
                    .px_4()
                    .gap_4()
                    .justify_between()
                    .border_b_1()
                    .border_color(outline_variant())
                    .child(
                        h_flex()
                            .min_w_0()
                            .gap_4()
                            .child(div().text_base().font_semibold().child("Response"))
                            .child(self.render_response_summary(response, cx)),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .when(self.response_tab == ResponseTab::Body, |this| {
                                this.child(
                                    Button::new("toggle-pretty")
                                        .label(if self.pretty_body { "Pretty" } else { "Raw" })
                                        .small()
                                        .ghost()
                                        .rounded(px(18.))
                                        .selected(self.pretty_body)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.pretty_body = !this.pretty_body;
                                            this.copied = false;
                                            if let Some(response) = this.response.clone() {
                                                this.update_response_editor(&response, window, cx);
                                            }
                                            cx.notify();
                                        })),
                                )
                            })
                            .when(self.response_tab != ResponseTab::Scripts, |this| {
                                this.child(
                                    Button::new("copy-response")
                                        .label(if self.copied { "Copied" } else { "Copy" })
                                        .small()
                                        .ghost()
                                        .rounded(px(18.))
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.copy_response(cx)),
                                        ),
                                )
                            }),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .h(px(42.))
                    .flex_shrink_0()
                    .px_4()
                    .justify_between()
                    .border_b_1()
                    .border_color(outline_variant())
                    .child(
                        TabBar::new("response-tabs")
                            .underline()
                            .children(["Body", "Headers", "Preview", "Scripts"])
                            .selected_index(self.response_tab.index())
                            .on_click(cx.listener(|this, index: &usize, window, cx| {
                                this.select_response_tab(*index, window, cx);
                            })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .ml_3()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_right()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(compact_url(&response.final_url)),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .p_4()
                    .when(self.response_tab == ResponseTab::Body, |this| {
                        this.child(self.render_response_body(cx))
                    })
                    .when(self.response_tab == ResponseTab::Headers, |this| {
                        this.child(self.render_response_headers(response, cx))
                    })
                    .when(self.response_tab == ResponseTab::Preview, |this| {
                        this.child(self.render_preview(response, cx))
                    })
                    .when(self.response_tab == ResponseTab::Scripts, |this| {
                        this.child(self.render_script_results(cx))
                    }),
            )
            .into_any_element()
    }
}

impl Render for ApiTester {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.debug_overlay.read(cx).record_ui_frame();

        v_flex()
            .size_full()
            .relative()
            .overflow_hidden()
            .bg(surface())
            .text_color(cx.theme().foreground)
            .capture_key_down(cx.listener(Self::capture_template_key_down))
            .child(self.render_title_bar(cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_navigation_rail(cx))
                    .child(
                        div()
                            .h_full()
                            .flex_1()
                            .min_w_0()
                            .when(self.sidebar_tab == SidebarTab::Environments, |this| {
                                this.child(self.render_environment_workspace(cx))
                            })
                            .when(self.sidebar_tab != SidebarTab::Environments, |this| {
                                this.child(
                                    h_resizable("workspace-split")
                                        .child(
                                            resizable_panel()
                                                .size(px(360.))
                                                .size_range(px(320.)..px(480.))
                                                .child(self.render_sidebar(cx)),
                                        )
                                        .child(
                                            resizable_panel().child(
                                                v_resizable("request-response-split")
                                                    .child(
                                                        resizable_panel()
                                                            .size(px(480.))
                                                            .size_range(px(360.)..px(900.))
                                                            .child(self.render_request_panel(cx)),
                                                    )
                                                    .child(
                                                        resizable_panel()
                                                            .size_range(px(240.)..px(1_400.))
                                                            .child(self.render_response_panel(cx)),
                                                    ),
                                            ),
                                        ),
                                )
                            }),
                    ),
            )
            .child(self.debug_overlay.clone())
            .children(self.render_template_variable_popover(cx))
            .children(Root::render_dialog_layer(window, cx))
    }
}

fn input_text_equals(input: &Entity<InputState>, expected: &str, cx: &App) -> bool {
    let input = input.read(cx);
    let text = input.text();
    text.len() == expected.len() && text.chars().eq(expected.chars())
}

fn input_text_is_blank(input: &Entity<InputState>, cx: &App) -> bool {
    input.read(cx).text().chars().all(char::is_whitespace)
}

fn compact_url(url: &str) -> String {
    const MAX_CHARS: usize = 64;
    if url.chars().count() <= MAX_CHARS {
        return url.to_owned();
    }
    let prefix: String = url.chars().take(MAX_CHARS - 1).collect();
    format!("{prefix}…")
}

fn unique_name<'a>(base: &str, existing: impl IntoIterator<Item = &'a str>) -> String {
    let existing = existing.into_iter().collect::<Vec<_>>();
    if !existing.iter().any(|name| name.eq_ignore_ascii_case(base)) {
        return base.to_owned();
    }

    for suffix in 2.. {
        let candidate = format!("{base} {suffix}");
        if !existing
            .iter()
            .any(|name| name.eq_ignore_ascii_case(&candidate))
        {
            return candidate;
        }
    }
    unreachable!("an unused numeric suffix must exist")
}

fn default_request_name(request: &RequestDraft) -> String {
    let endpoint = url::Url::parse(request.url.trim())
        .ok()
        .and_then(|url| {
            let host = url.host_str()?.to_owned();
            let path = url.path().trim_matches('/');
            Some(if path.is_empty() {
                host
            } else {
                format!("{host}/{path}")
            })
        })
        .unwrap_or_else(|| compact_url(request.url.trim()));
    format!(
        "{} {}",
        request.method.trim().to_ascii_uppercase(),
        endpoint
    )
    .trim()
    .to_owned()
}

fn code_language_for_raw_body(language: RawBodyLanguage) -> CodeLanguage {
    match language {
        RawBodyLanguage::Text => CodeLanguage::Plain,
        RawBodyLanguage::Json => CodeLanguage::Json,
        RawBodyLanguage::Xml => CodeLanguage::Html,
        RawBodyLanguage::Html => CodeLanguage::Html,
        RawBodyLanguage::JavaScript => CodeLanguage::JavaScript,
        RawBodyLanguage::TypeScript => CodeLanguage::TypeScript,
        RawBodyLanguage::Css => CodeLanguage::Css,
        RawBodyLanguage::Markdown => CodeLanguage::Markdown,
        RawBodyLanguage::GraphQl => CodeLanguage::GraphQl,
        RawBodyLanguage::Yaml => CodeLanguage::Yaml,
        RawBodyLanguage::Toml => CodeLanguage::Toml,
        RawBodyLanguage::Sql => CodeLanguage::Sql,
        RawBodyLanguage::Shell => CodeLanguage::Shell,
        RawBodyLanguage::Rust => CodeLanguage::Rust,
        RawBodyLanguage::Python => CodeLanguage::Python,
    }
}

fn update_script_variable_catalog(
    catalog: &Rc<RefCell<ScriptVariableCatalog>>,
    workspace: &Workspace,
) {
    let enabled_names = workspace
        .active_environment()
        .into_iter()
        .flat_map(|environment| environment.variables.iter())
        .filter(|variable| variable.enabled)
        .map(|variable| variable.key.clone())
        .collect::<Vec<_>>();
    let disabled_names = workspace
        .active_environment()
        .into_iter()
        .flat_map(|environment| environment.variables.iter())
        .filter(|variable| !variable.enabled)
        .map(|variable| variable.key.clone())
        .collect::<Vec<_>>();
    catalog
        .borrow_mut()
        .replace(enabled_names, disabled_names, std::iter::empty::<String>());
}

fn template_input_state(
    window: &mut Window,
    cx: &mut Context<InputState>,
    catalog: TemplateVariableCatalogHandle,
    placeholder: impl Into<SharedString>,
    default_value: impl Into<SharedString>,
) -> InputState {
    let hover_catalog = Rc::clone(&catalog);
    let mut state = InputState::new(window, cx)
        .placeholder(placeholder)
        .default_value(default_value);
    state.lsp.completion_provider = Some(Rc::new(TemplateCompletionProvider::new(catalog)));
    state.lsp.hover_provider = Some(Rc::new(TemplateHoverProvider::new(hover_catalog)));
    state
}

fn format_raw_body_source(language: RawBodyLanguage, source: &str) -> Result<String, String> {
    match language {
        RawBodyLanguage::Json => serde_json::from_str::<serde_json::Value>(source)
            .and_then(|value| serde_json::to_string_pretty(&value))
            .map_err(|error| format!("JSON could not be formatted: {error}")),
        language => Err(format!(
            "Format buffer is not available for {language} yet. The buffer was not changed."
        )),
    }
}

fn response_language(response: &ResponseData) -> CodeLanguage {
    let content_type = response
        .content_type
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();

    if content_type.contains("json") {
        CodeLanguage::Json
    } else if content_type.contains("javascript") || content_type.contains("ecmascript") {
        CodeLanguage::JavaScript
    } else if content_type.contains("html") {
        CodeLanguage::Html
    } else if content_type.contains("css") {
        CodeLanguage::Css
    } else if content_type.contains("markdown") {
        CodeLanguage::Markdown
    } else if content_type.contains("yaml") {
        CodeLanguage::Yaml
    } else if content_type.contains("toml") {
        CodeLanguage::Toml
    } else if content_type.contains("xml") {
        CodeLanguage::Html
    } else if serde_json::from_slice::<serde_json::Value>(&response.body).is_ok() {
        CodeLanguage::Json
    } else {
        CodeLanguage::from("text")
    }
}

fn compact_label(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_owned();
    }
    let visible = max_chars.saturating_sub(1);
    let head = visible.saturating_mul(2) / 3;
    let tail = visible.saturating_sub(head);
    let prefix = value.chars().take(head).collect::<String>();
    let suffix = value
        .chars()
        .rev()
        .take(tail)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    format!("{prefix}…{suffix}")
}

impl ScriptConsoleModel {
    fn row_count(&self) -> usize {
        self.sections.iter().map(|section| section.rows.len()).sum()
    }

    fn copy_all_text(&self) -> String {
        if self.sections.is_empty() {
            return "No script has run yet.".to_owned();
        }

        self.sections
            .iter()
            .map(|section| {
                let mut lines = vec![match section.duration {
                    Some(duration) => {
                        format!("{} · {}", section.title, format_script_duration(duration))
                    }
                    None => section.title.clone(),
                }];
                lines.extend(section.rows.iter().map(|row| {
                    let mut text = format!("[{}] {}", row.label, row.message);
                    if let Some(detail) = row.detail.as_deref() {
                        for line in detail.lines() {
                            text.push_str("\n    ");
                            text.push_str(line);
                        }
                    }
                    text
                }));
                lines.join("\n")
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

fn script_console_model(
    pre: Option<&ScriptReport>,
    post: Option<&ScriptReport>,
    diagnostic: Option<&ScriptDiagnostic>,
    request_error: Option<&str>,
) -> ScriptConsoleModel {
    let mut sections = Vec::new();
    let mut diagnostic_rendered = false;

    for report in [pre, post].into_iter().flatten() {
        let mut rows = Vec::new();

        for log in &report.logs {
            let (label, tone) = match log.level {
                ScriptLogLevel::Log => ("LOG", ScriptConsoleTone::Neutral),
                ScriptLogLevel::Info => ("INFO", ScriptConsoleTone::Info),
                ScriptLogLevel::Warn => ("WARN", ScriptConsoleTone::Warning),
                ScriptLogLevel::Error => ("ERROR", ScriptConsoleTone::Danger),
                ScriptLogLevel::Debug => ("DEBUG", ScriptConsoleTone::Debug),
            };
            rows.push(ScriptConsoleRow {
                label: label.to_owned(),
                message: log.message.clone(),
                detail: None,
                copy_value: log.message.clone(),
                tone,
            });
        }

        for test in &report.tests {
            let label = if test.passed { "PASS" } else { "FAIL" };
            let mut copy_value = format!("[{label}] {}", test.name);
            if let Some(message) = test.message.as_deref() {
                copy_value.push('\n');
                copy_value.push_str(message);
            }
            rows.push(ScriptConsoleRow {
                label: label.to_owned(),
                message: test.name.clone(),
                detail: test.message.clone(),
                copy_value,
                tone: if test.passed {
                    ScriptConsoleTone::Success
                } else {
                    ScriptConsoleTone::Danger
                },
            });
        }

        if report.response_body_truncated {
            let message = "Response body was truncated for the script runtime.".to_owned();
            rows.push(ScriptConsoleRow {
                label: "NOTICE".to_owned(),
                message: message.clone(),
                detail: None,
                copy_value: message,
                tone: ScriptConsoleTone::Warning,
            });
        }

        if let Some(diagnostic) = diagnostic.filter(|item| item.phase == report.phase) {
            rows.push(script_diagnostic_console_row(diagnostic));
            diagnostic_rendered = true;
        }

        if rows.is_empty() {
            let message = "No console output or tests.".to_owned();
            rows.push(ScriptConsoleRow {
                label: "EMPTY".to_owned(),
                message: message.clone(),
                detail: None,
                copy_value: message,
                tone: ScriptConsoleTone::Neutral,
            });
        }

        sections.push(ScriptConsoleSection {
            key: report.phase.to_string(),
            title: script_phase_title(report.phase).to_owned(),
            duration: Some(report.duration),
            rows,
        });
    }

    if let Some(diagnostic) = diagnostic.filter(|_| !diagnostic_rendered) {
        sections.push(ScriptConsoleSection {
            key: diagnostic.phase.to_string(),
            title: script_phase_title(diagnostic.phase).to_owned(),
            duration: None,
            rows: vec![script_diagnostic_console_row(diagnostic)],
        });
    }

    // Script failures already carry a structured diagnostic. Avoid presenting
    // the same error a second time through the request-level fallback string.
    if diagnostic.is_none()
        && let Some(error) = request_error.filter(|error| !error.trim().is_empty())
    {
        sections.push(ScriptConsoleSection {
            key: "request".to_owned(),
            title: "Request".to_owned(),
            duration: None,
            rows: vec![ScriptConsoleRow {
                label: "ERROR".to_owned(),
                message: error.to_owned(),
                detail: None,
                copy_value: error.to_owned(),
                tone: ScriptConsoleTone::Danger,
            }],
        });
    }

    ScriptConsoleModel { sections }
}

fn script_diagnostic_console_row(diagnostic: &ScriptDiagnostic) -> ScriptConsoleRow {
    let label = script_error_kind_label(diagnostic.kind);
    let message = format!("{}: {}", diagnostic.filename, diagnostic.message);
    let mut copy_value = format!("[{label}] {message}");
    if let Some(stack) = diagnostic.stack.as_deref() {
        copy_value.push('\n');
        copy_value.push_str(stack);
    }
    ScriptConsoleRow {
        label: label.to_owned(),
        message,
        detail: diagnostic.stack.clone(),
        copy_value,
        tone: ScriptConsoleTone::Danger,
    }
}

fn script_phase_title(phase: ScriptPhase) -> &'static str {
    match phase {
        ScriptPhase::PreRequest => "Pre-request",
        ScriptPhase::PostResponse => "Post-response",
    }
}

fn script_error_kind_label(kind: ScriptErrorKind) -> &'static str {
    match kind {
        ScriptErrorKind::SourceLimit => "SOURCE",
        ScriptErrorKind::BodyLimit => "BODY",
        ScriptErrorKind::Cancelled => "CANCEL",
        ScriptErrorKind::TimedOut => "TIMEOUT",
        ScriptErrorKind::MemoryLimit => "MEMORY",
        ScriptErrorKind::Syntax => "SYNTAX",
        ScriptErrorKind::Runtime => "RUNTIME",
        ScriptErrorKind::OutputLimit => "OUTPUT",
        ScriptErrorKind::Engine => "ENGINE",
    }
}

fn format_script_duration(duration: Duration) -> String {
    format!("{:.2} ms", duration.as_secs_f64() * 1_000.0)
}

fn script_console_tone_color(tone: ScriptConsoleTone, cx: &App) -> Hsla {
    match tone {
        ScriptConsoleTone::Neutral | ScriptConsoleTone::Debug => cx.theme().muted_foreground,
        ScriptConsoleTone::Info => cx.theme().info,
        ScriptConsoleTone::Warning => cx.theme().warning,
        ScriptConsoleTone::Danger => cx.theme().danger,
        ScriptConsoleTone::Success => cx.theme().success,
    }
}

fn script_console_tone_background(tone: ScriptConsoleTone, cx: &App) -> Hsla {
    match tone {
        ScriptConsoleTone::Info => cx.theme().info.opacity(0.025),
        ScriptConsoleTone::Warning => cx.theme().warning.opacity(0.045),
        ScriptConsoleTone::Danger => cx.theme().danger.opacity(0.055),
        ScriptConsoleTone::Success => cx.theme().success.opacity(0.025),
        ScriptConsoleTone::Neutral | ScriptConsoleTone::Debug => surface_lowest(),
    }
}

fn script_console_tone_icon(tone: ScriptConsoleTone) -> IconName {
    match tone {
        ScriptConsoleTone::Neutral => IconName::SquareTerminal,
        ScriptConsoleTone::Info => IconName::Info,
        ScriptConsoleTone::Warning => IconName::TriangleAlert,
        ScriptConsoleTone::Danger => IconName::CircleX,
        ScriptConsoleTone::Success => IconName::CircleCheck,
        ScriptConsoleTone::Debug => IconName::Inspector,
    }
}

fn format_duration(duration: std::time::Duration) -> String {
    if duration.as_secs() >= 1 {
        format!("{:.2} s", duration.as_secs_f64())
    } else {
        format!("{} ms", duration.as_millis())
    }
}

fn format_bytes(bytes: usize) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    if bytes as f64 >= MIB {
        format!("{:.1} MiB", bytes as f64 / MIB)
    } else if bytes as f64 >= KIB {
        format!("{:.1} KiB", bytes as f64 / KIB)
    } else {
        format!("{bytes} B")
    }
}

fn method_color(method: &str, cx: &App) -> Hsla {
    match method.trim().to_ascii_uppercase().as_str() {
        "GET" => cx.theme().green,
        "POST" => cx.theme().yellow,
        "PUT" => cx.theme().blue,
        "PATCH" => cx.theme().magenta,
        "DELETE" => cx.theme().red,
        "HEAD" => cx.theme().cyan,
        "OPTIONS" => cx.theme().magenta_light,
        _ => cx.theme().cyan,
    }
}

fn status_color(status: u16, cx: &App) -> Hsla {
    match status {
        200..=299 => cx.theme().success,
        300..=399 => cx.theme().info,
        400..=499 => cx.theme().warning,
        _ => cx.theme().danger,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{ScriptLog, ScriptTestResult};

    #[test]
    fn raw_json_formatter_pretty_prints_without_changing_values() {
        let formatted = format_raw_body_source(
            RawBodyLanguage::Json,
            r#"{"nested":{"ok":true},"items":[1,2]}"#,
        )
        .unwrap();
        assert_eq!(
            formatted,
            "{\n  \"nested\": {\n    \"ok\": true\n  },\n  \"items\": [\n    1,\n    2\n  ]\n}"
        );
    }

    #[test]
    fn raw_formatter_does_not_guess_for_unsupported_languages() {
        let source = "name: value";
        let error = format_raw_body_source(RawBodyLanguage::Yaml, source).unwrap_err();
        assert!(error.contains("not available for YAML"));
        assert_eq!(source, "name: value");
    }

    #[test]
    fn script_console_builds_level_and_test_rows_with_stable_copy_text() {
        let report = ScriptReport {
            phase: ScriptPhase::PreRequest,
            duration: Duration::from_micros(420),
            logs: vec![
                ScriptLog {
                    level: ScriptLogLevel::Log,
                    message: "plain".to_owned(),
                },
                ScriptLog {
                    level: ScriptLogLevel::Info,
                    message: "bilgi 🧪".to_owned(),
                },
                ScriptLog {
                    level: ScriptLogLevel::Warn,
                    message: "careful".to_owned(),
                },
                ScriptLog {
                    level: ScriptLogLevel::Error,
                    message: "boom".to_owned(),
                },
                ScriptLog {
                    level: ScriptLogLevel::Debug,
                    message: "details".to_owned(),
                },
            ],
            tests: vec![
                ScriptTestResult {
                    name: "created".to_owned(),
                    passed: true,
                    message: None,
                },
                ScriptTestResult {
                    name: "has token".to_owned(),
                    passed: false,
                    message: Some("expected value\nreceived none".to_owned()),
                },
            ],
            response_body_truncated: true,
        };

        let model = script_console_model(Some(&report), None, None, None);
        assert_eq!(
            model.sections[0]
                .rows
                .iter()
                .map(|row| row.label.as_str())
                .collect::<Vec<_>>(),
            [
                "LOG", "INFO", "WARN", "ERROR", "DEBUG", "PASS", "FAIL", "NOTICE"
            ]
        );
        assert_eq!(model.sections[0].rows[1].copy_value, "bilgi 🧪");
        assert_eq!(
            model.sections[0].rows[6].copy_value,
            "[FAIL] has token\nexpected value\nreceived none"
        );
        assert_eq!(
            model.copy_all_text(),
            concat!(
                "Pre-request · 0.42 ms\n",
                "[LOG] plain\n",
                "[INFO] bilgi 🧪\n",
                "[WARN] careful\n",
                "[ERROR] boom\n",
                "[DEBUG] details\n",
                "[PASS] created\n",
                "[FAIL] has token\n",
                "    expected value\n",
                "    received none\n",
                "[NOTICE] Response body was truncated for the script runtime."
            )
        );
    }

    #[test]
    fn script_console_keeps_diagnostics_copyable_without_an_http_response() {
        let report = ScriptReport {
            phase: ScriptPhase::PreRequest,
            duration: Duration::from_millis(3),
            logs: vec![ScriptLog {
                level: ScriptLogLevel::Info,
                message: "before failure".to_owned(),
            }],
            tests: Vec::new(),
            response_body_truncated: false,
        };
        let diagnostic = ScriptDiagnostic {
            phase: ScriptPhase::PreRequest,
            kind: ScriptErrorKind::Runtime,
            filename: "pre-request.js",
            message: "patladı".to_owned(),
            stack: Some("at pre-request.js:4\nat <eval>".to_owned()),
        };

        let model = script_console_model(
            Some(&report),
            None,
            Some(&diagnostic),
            Some("duplicate fallback error"),
        );
        assert_eq!(model.sections.len(), 1);
        assert_eq!(model.sections[0].rows.len(), 2);
        assert_eq!(
            model.sections[0].rows[1].copy_value,
            "[RUNTIME] pre-request.js: patladı\nat pre-request.js:4\nat <eval>"
        );
        let copied = model.copy_all_text();
        assert!(copied.contains("[INFO] before failure"));
        assert!(copied.contains("[RUNTIME] pre-request.js: patladı"));
        assert!(copied.contains("    at pre-request.js:4"));
        assert!(!copied.contains("duplicate fallback error"));
    }

    #[test]
    fn script_console_includes_network_failure_after_pre_script_output() {
        let report = ScriptReport {
            phase: ScriptPhase::PreRequest,
            duration: Duration::from_millis(1),
            logs: Vec::new(),
            tests: Vec::new(),
            response_body_truncated: false,
        };

        let model = script_console_model(Some(&report), None, None, Some("connection refused"));
        assert_eq!(model.sections.len(), 2);
        assert_eq!(model.sections[0].rows[0].label, "EMPTY");
        assert_eq!(model.sections[1].title, "Request");
        assert_eq!(model.sections[1].rows[0].copy_value, "connection refused");
        assert!(model.copy_all_text().contains("[ERROR] connection refused"));
    }

    #[test]
    fn script_console_keeps_pre_request_before_post_response() {
        let pre = ScriptReport {
            phase: ScriptPhase::PreRequest,
            duration: Duration::from_millis(1),
            logs: vec![ScriptLog {
                level: ScriptLogLevel::Log,
                message: "pre".to_owned(),
            }],
            tests: Vec::new(),
            response_body_truncated: false,
        };
        let post = ScriptReport {
            phase: ScriptPhase::PostResponse,
            duration: Duration::from_millis(2),
            logs: vec![ScriptLog {
                level: ScriptLogLevel::Info,
                message: "post".to_owned(),
            }],
            tests: Vec::new(),
            response_body_truncated: false,
        };

        let model = script_console_model(Some(&pre), Some(&post), None, None);
        assert_eq!(
            model
                .sections
                .iter()
                .map(|section| section.title.as_str())
                .collect::<Vec<_>>(),
            ["Pre-request", "Post-response"]
        );
        assert_eq!(
            model.copy_all_text(),
            "Pre-request · 1.00 ms\n[LOG] pre\n\n\
             Post-response · 2.00 ms\n[INFO] post"
        );
    }

    #[test]
    fn empty_script_console_has_a_copyable_empty_state() {
        let model = script_console_model(None, None, None, None);
        assert_eq!(model.row_count(), 0);
        assert_eq!(model.copy_all_text(), "No script has run yet.");
    }
}

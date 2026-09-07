//! Lightweight editor intelligence for the Resolved script runtime.
//!
//! Selected-environment values are available to the real request-script
//! editors, where they help authors confirm the configured value behind a
//! variable name. Plain snippets keep the same variable-name completion
//! without exposing values. Values never enter diagnostics or `Debug` output.

use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    fmt,
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
};

use anyhow::Result;
use gpui::{App, AppContext as _, Context, Task, Window};
use gpui_component::input::{CompletionProvider, HoverProvider, InputState, Rope};
use lsp_types::{
    CompletionContext, CompletionItem, CompletionItemKind, CompletionResponse, CompletionTextEdit,
    Diagnostic, DiagnosticSeverity, Documentation, Hover, HoverContents, MarkupContent, MarkupKind,
    NumberOrString, TextEdit,
};

use crate::{
    core::{BodyFieldKind, BodyMode, RawBodyLanguage, STANDARD_HTTP_METHODS},
    editor_util::{clipped_char_boundary, source_range},
    typescript_service::{TypeScriptDocumentKind, TypeScriptScriptPhase, TypeScriptServiceHandle},
};

const DIAGNOSTIC_SOURCE: &str = "Resolved";
const MISSING_VARIABLE_CODE: &str = "missing-script-variable";
const DISABLED_VARIABLE_CODE: &str = "disabled-script-variable";
const COMPLETION_CONTEXT_LIMIT: usize = 512;
const VARIABLE_CONTEXT_PADDING: usize = 96;

/// The script editor currently being completed.
///
/// Runtime members that cannot be used in a phase are omitted from completion
/// results. In particular, response inspection and test helpers are available
/// only after a response exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScriptEditorPhase {
    PreRequest,
    PostResponse,
}

const fn typescript_phase(phase: ScriptEditorPhase) -> TypeScriptScriptPhase {
    match phase {
        ScriptEditorPhase::PreRequest => TypeScriptScriptPhase::PreRequest,
        ScriptEditorPhase::PostResponse => TypeScriptScriptPhase::PostResponse,
    }
}

/// A snapshot of variables visible to a script.
///
/// Values are private and its `Debug` implementation deliberately reports
/// only their count. Providers decide whether their editor may display them.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct ScriptVariableCatalog {
    environment_names: BTreeSet<String>,
    environment_values: BTreeMap<String, String>,
    disabled_environment_names: BTreeSet<String>,
    collection_names: BTreeSet<String>,
}

impl fmt::Debug for ScriptVariableCatalog {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ScriptVariableCatalog")
            .field("environment_names", &self.environment_names)
            .field("environment_value_count", &self.environment_values.len())
            .field(
                "disabled_environment_names",
                &self.disabled_environment_names,
            )
            .field("collection_names", &self.collection_names)
            .finish()
    }
}

impl ScriptVariableCatalog {
    /// Builds a catalog that distinguishes enabled and disabled names.
    ///
    /// Disabled names are retained only for diagnostics. They are not offered
    /// as completions and are not considered available to the script runtime.
    pub fn from_names<E, D, C, EN, DN, CN>(
        environment_names: E,
        disabled_environment_names: D,
        collection_names: C,
    ) -> Self
    where
        E: IntoIterator<Item = EN>,
        D: IntoIterator<Item = DN>,
        C: IntoIterator<Item = CN>,
        EN: Into<String>,
        DN: Into<String>,
        CN: Into<String>,
    {
        let environment_names = normalized_names(environment_names);
        let mut disabled_environment_names = normalized_names(disabled_environment_names);
        disabled_environment_names.retain(|name| !environment_names.contains(name));
        Self {
            environment_names,
            environment_values: BTreeMap::new(),
            disabled_environment_names,
            collection_names: normalized_names(collection_names),
        }
    }

    /// Builds a catalog with the current values of enabled variables in the
    /// selected environment. Values remain redacted from `Debug` output.
    pub fn from_environment_values<E, D, C, K, V, DN, CN>(
        environment_variables: E,
        disabled_environment_names: D,
        collection_names: C,
    ) -> Self
    where
        E: IntoIterator<Item = (K, V)>,
        D: IntoIterator<Item = DN>,
        C: IntoIterator<Item = CN>,
        K: Into<String>,
        V: Into<String>,
        DN: Into<String>,
        CN: Into<String>,
    {
        let environment_values = environment_variables
            .into_iter()
            .map(|(name, value)| (name.into(), value.into()))
            .filter(|(name, _)| !name.is_empty())
            .collect::<BTreeMap<_, _>>();
        let mut catalog = Self::from_names(
            environment_values.keys().cloned(),
            disabled_environment_names,
            collection_names,
        );
        catalog.environment_values = environment_values;
        catalog
    }

    pub fn environment_names(&self) -> impl ExactSizeIterator<Item = &str> {
        self.environment_names.iter().map(String::as_str)
    }

    /// Names visible through `api.variables`, in deterministic display order.
    pub fn variable_names(&self) -> impl Iterator<Item = &str> {
        self.environment_names
            .union(&self.collection_names)
            .map(String::as_str)
    }

    /// Replaces the snapshot with names only. Providers sharing this catalog
    /// see the selected-environment change on their next invocation.
    pub fn replace<E, D, C, EN, DN, CN>(
        &mut self,
        environment_names: E,
        disabled_environment_names: D,
        collection_names: C,
    ) where
        E: IntoIterator<Item = EN>,
        D: IntoIterator<Item = DN>,
        C: IntoIterator<Item = CN>,
        EN: Into<String>,
        DN: Into<String>,
        CN: Into<String>,
    {
        *self = Self::from_names(
            environment_names,
            disabled_environment_names,
            collection_names,
        );
    }

    /// Replaces the snapshot with current selected-environment values.
    pub fn replace_with_environment_values<E, D, C, K, V, DN, CN>(
        &mut self,
        environment_variables: E,
        disabled_environment_names: D,
        collection_names: C,
    ) where
        E: IntoIterator<Item = (K, V)>,
        D: IntoIterator<Item = DN>,
        C: IntoIterator<Item = CN>,
        K: Into<String>,
        V: Into<String>,
        DN: Into<String>,
        CN: Into<String>,
    {
        let replacement = Self::from_environment_values(
            environment_variables,
            disabled_environment_names,
            collection_names,
        );
        let environment_values = replacement.environment_values;
        self.replace(
            replacement.environment_names,
            replacement.disabled_environment_names,
            replacement.collection_names,
        );
        self.environment_values = environment_values;
    }

    pub fn shared(self) -> ScriptVariableCatalogHandle {
        Rc::new(RefCell::new(self))
    }
}

/// Shared, live catalog used by both script editors.
pub type ScriptVariableCatalogHandle = Rc<RefCell<ScriptVariableCatalog>>;

fn normalized_names<I, N>(names: I) -> BTreeSet<String>
where
    I: IntoIterator<Item = N>,
    N: Into<String>,
{
    names
        .into_iter()
        .map(Into::into)
        .filter(|name| !name.is_empty())
        .collect()
}

/// Completion provider for the sandboxed pre-request/post-response API.
#[derive(Clone)]
pub struct ScriptCompletionProvider {
    phase: ScriptEditorPhase,
    variables: ScriptVariableCatalogHandle,
    expose_environment_values: bool,
    typescript: Option<TypeScriptServiceHandle>,
    typescript_document: TypeScriptDocumentKind,
    request_namespace: Option<ScriptRequestNamespaceHandle>,
    websocket_project: Option<Rc<RefCell<crate::typescript_service::WebSocketScriptProject>>>,
    document: Rc<RefCell<ScriptDocumentVersion>>,
}

/// Shared, live saved-request namespace model used by both script editors. It
/// is refreshed from the active workspace whenever the workspace changes, so
/// the editor and the script runtime can never drift (the runtime injects the
/// very same catalog).
pub type ScriptRequestNamespaceHandle = Rc<RefCell<crate::core::RequestNamespaceCatalog>>;

#[derive(Default)]
struct ScriptDocumentVersion {
    source: Option<String>,
    version: u64,
}

static NEXT_SCRIPT_DOCUMENT_VERSION: AtomicU64 = AtomicU64::new(1);

impl ScriptCompletionProvider {
    pub fn new(phase: ScriptEditorPhase, variables: ScriptVariableCatalogHandle) -> Self {
        Self {
            phase,
            variables,
            expose_environment_values: true,
            typescript: None,
            typescript_document: TypeScriptDocumentKind::Script(typescript_phase(phase)),
            request_namespace: None,
            websocket_project: None,
            document: Rc::new(RefCell::new(ScriptDocumentVersion::default())),
        }
    }

    /// Builds request-script intelligence for a plain snippet without sharing
    /// the live request editor's TypeScript document slot.
    pub fn for_plain_snippet(
        phase: ScriptEditorPhase,
        variables: ScriptVariableCatalogHandle,
    ) -> Self {
        Self {
            expose_environment_values: false,
            typescript_document: TypeScriptDocumentKind::PlainSnippet(typescript_phase(phase)),
            ..Self::new(phase, variables)
        }
    }

    pub fn for_websocket_automation(
        project: Rc<RefCell<crate::typescript_service::WebSocketScriptProject>>,
    ) -> Self {
        Self {
            typescript_document: TypeScriptDocumentKind::WebSocketAutomation,
            websocket_project: Some(project),
            ..Self::new(
                ScriptEditorPhase::PostResponse,
                Rc::new(RefCell::new(ScriptVariableCatalog::default())),
            )
        }
    }

    pub fn for_interactive_console(variables: ScriptVariableCatalogHandle) -> Self {
        Self {
            typescript_document: TypeScriptDocumentKind::InteractiveConsole,
            ..Self::new(ScriptEditorPhase::PostResponse, variables)
        }
    }

    fn typescript_service(&self) -> Option<TypeScriptServiceHandle> {
        self.typescript
            .clone()
            .map(|service| match &self.websocket_project {
                Some(project) => service.with_websocket_project(project.borrow().clone()),
                None => service,
            })
    }

    /// Adds Microsoft's embedded TypeScript Language Service for ordinary
    /// JavaScript semantics. Resolved-specific API and live variable
    /// completion remains a narrow overlay.
    pub fn with_typescript_service(mut self, typescript: TypeScriptServiceHandle) -> Self {
        self.typescript = Some(typescript);
        self
    }

    /// Shares the live saved-request namespace model from the active workspace
    /// so collection-tree completion/hover/diagnostics match the runtime.
    pub fn with_request_namespace(mut self, namespace: ScriptRequestNamespaceHandle) -> Self {
        self.request_namespace = Some(namespace);
        self
    }

    /// Synchronous completion entrypoint used by the GPUI provider and focused
    /// unit tests.
    pub fn completion_items_for_source(&self, source: &str, offset: usize) -> Vec<CompletionItem> {
        if self.typescript_document == TypeScriptDocumentKind::WebSocketAutomation {
            return Vec::new();
        }
        let namespace = self
            .request_namespace
            .as_ref()
            .map(|handle| handle.borrow().clone());
        completion_items(
            source,
            offset,
            self.phase,
            &self.variables.borrow(),
            self.expose_environment_values,
            namespace.as_ref(),
        )
    }

    /// Returns documentation for the runtime symbol or variable name under
    /// the pointer.
    pub fn hover_for_source(&self, source: &str, offset: usize) -> Option<Hover> {
        if self.typescript_document == TypeScriptDocumentKind::WebSocketAutomation {
            return None;
        }
        let namespace = self
            .request_namespace
            .as_ref()
            .map(|handle| handle.borrow().clone());
        hover_for_source(
            source,
            offset,
            self.phase,
            &self.variables.borrow(),
            self.expose_environment_values,
            namespace.as_ref(),
        )
    }

    /// Produces semantic JavaScript diagnostics asynchronously and merges the
    /// result with Resolved's variable warnings.
    pub fn diagnostics_task(&self, source: String, cx: &mut App) -> Task<Vec<Diagnostic>> {
        let variables = self.variables.borrow().clone();
        let namespace = self
            .request_namespace
            .as_ref()
            .map(|handle| handle.borrow().clone());
        let typescript_request = self.typescript_service().map(|typescript| {
            (
                typescript,
                self.typescript_document,
                self.version_for_source(&source),
            )
        });

        let websocket = self.typescript_document == TypeScriptDocumentKind::WebSocketAutomation;
        cx.background_spawn(async move {
            let local = if websocket {
                Vec::new()
            } else {
                diagnostics_for_source(&source, &variables, namespace.as_ref())
            };
            let Some((typescript, document, version)) = typescript_request else {
                return local;
            };
            match typescript.diagnostics(document, version, source).await {
                Ok(typescript_diagnostics) => merge_diagnostics(local, typescript_diagnostics),
                Err(error) => {
                    tracing::debug!(%error, "TypeScript diagnostics unavailable");
                    local
                }
            }
        })
    }

    fn version_for_source(&self, source: &str) -> u64 {
        let mut document = self.document.borrow_mut();
        if document.source.as_deref() != Some(source) {
            document.source = Some(source.to_owned());
            document.version = NEXT_SCRIPT_DOCUMENT_VERSION.fetch_add(1, Ordering::Relaxed);
        }
        document.version
    }
}

impl CompletionProvider for ScriptCompletionProvider {
    fn supports_inline_completion(&self) -> bool {
        false
    }

    fn completions(
        &self,
        text: &Rope,
        offset: usize,
        _trigger: CompletionContext,
        _window: &mut Window,
        cx: &mut Context<InputState>,
    ) -> Task<Result<CompletionResponse>> {
        let source = text.to_string();
        let Some(typescript) = self.typescript_service() else {
            let local = self.completion_items_for_source(&source, offset);
            return Task::ready(Ok(CompletionResponse::Array(local)));
        };
        let variables = self.variables.borrow().clone();
        let local_phase = self.phase;
        let expose_environment_values = self.expose_environment_values;
        let namespace = self
            .request_namespace
            .as_ref()
            .map(|handle| handle.borrow().clone());
        let document = self.typescript_document;
        let version = self.version_for_source(&source);

        cx.background_spawn(async move {
            let local = if document == TypeScriptDocumentKind::WebSocketAutomation {
                Vec::new()
            } else {
                completion_items(
                    &source,
                    offset,
                    local_phase,
                    &variables,
                    expose_environment_values,
                    namespace.as_ref(),
                )
            };
            let items = match typescript
                .completion_items(document, version, source, offset)
                .await
            {
                Ok(typescript_items) => merge_completion_items(local, typescript_items),
                Err(error) => {
                    tracing::debug!(%error, "TypeScript completion unavailable");
                    local
                }
            };
            Ok(CompletionResponse::Array(items))
        })
    }

    fn is_completion_trigger(
        &self,
        _offset: usize,
        new_text: &str,
        _cx: &mut Context<InputState>,
    ) -> bool {
        !new_text.is_empty()
            && new_text.chars().all(|character| {
                character.is_alphanumeric() || matches!(character, '_' | '$' | '.' | '"' | '\'')
            })
    }

    fn is_completion_trigger_in_text(
        &self,
        text: &Rope,
        cursor_offset: usize,
        _edit_start: usize,
        new_text: &str,
        _cx: &mut Context<InputState>,
    ) -> bool {
        if new_text.is_empty()
            || !new_text.chars().all(|character| {
                character.is_alphanumeric() || matches!(character, '_' | '$' | '.' | '"' | '\'')
            })
        {
            return false;
        }

        self.typescript.is_some()
            || script_completion_is_active(
                text,
                cursor_offset,
                self.phase,
                &self.variables.borrow(),
                self.request_namespace
                    .as_ref()
                    .map(|handle| handle.borrow().clone()),
            )
    }
}

impl HoverProvider for ScriptCompletionProvider {
    fn hover(
        &self,
        text: &Rope,
        offset: usize,
        _window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Option<Hover>>> {
        let source = text.to_string();
        let Some(typescript) = self.typescript_service() else {
            return Task::ready(Ok(self.hover_for_source(&source, offset)));
        };
        let variables = self.variables.borrow().clone();
        let local_phase = self.phase;
        let expose_environment_values = self.expose_environment_values;
        let namespace = self
            .request_namespace
            .as_ref()
            .map(|handle| handle.borrow().clone());
        let document = self.typescript_document;
        let version = self.version_for_source(&source);

        cx.background_spawn(async move {
            if document != TypeScriptDocumentKind::WebSocketAutomation
                && let Some(hover) = hover_for_source(
                    &source,
                    offset,
                    local_phase,
                    &variables,
                    expose_environment_values,
                    namespace.as_ref(),
                )
            {
                return Ok(Some(hover));
            }
            match typescript.hover(document, version, source, offset).await {
                Ok(hover) => Ok(hover),
                Err(error) => {
                    tracing::debug!(%error, "TypeScript hover unavailable");
                    Ok(None)
                }
            }
        })
    }
}

fn merge_completion_items(
    local: Vec<CompletionItem>,
    typescript: Vec<CompletionItem>,
) -> Vec<CompletionItem> {
    let mut labels = local
        .iter()
        .map(|item| item.label.clone())
        .collect::<BTreeSet<_>>();
    let mut merged = local;
    merged.extend(
        typescript
            .into_iter()
            .filter(|item| labels.insert(item.label.clone())),
    );
    merged
}

fn merge_diagnostics(local: Vec<Diagnostic>, typescript: Vec<Diagnostic>) -> Vec<Diagnostic> {
    let mut seen = BTreeSet::new();
    let mut merged = Vec::with_capacity(local.len() + typescript.len());
    for diagnostic in local.into_iter().chain(typescript) {
        let key = (
            diagnostic.range.start.line,
            diagnostic.range.start.character,
            diagnostic.range.end.line,
            diagnostic.range.end.character,
            diagnostic.message.clone(),
        );
        if seen.insert(key) {
            merged.push(diagnostic);
        }
    }
    merged
}

fn script_completion_is_active(
    text: &Rope,
    requested_cursor: usize,
    phase: ScriptEditorPhase,
    variables: &ScriptVariableCatalog,
    catalog: Option<crate::core::RequestNamespaceCatalog>,
) -> bool {
    let cursor = requested_cursor.min(text.len());
    let longest_variable_name = variables
        .variable_names()
        .map(|name| name.chars().count())
        .max()
        .unwrap_or_default();
    let scan_limit = COMPLETION_CONTEXT_LIMIT
        .max(longest_variable_name.saturating_add(VARIABLE_CONTEXT_PADDING));
    let mut characters = text
        .chars_at(cursor)
        .reversed()
        .take(scan_limit)
        .collect::<Vec<_>>();
    characters.reverse();
    let source = characters.into_iter().collect::<String>();
    let offset = source.len();
    let lexed = lex(&source);

    if lexed.ended_in_comment_or_template {
        return false;
    }

    if let Some(context) = request_literal_completion_context(&source, &lexed.tokens, offset, phase)
    {
        return context
            .kind
            .values()
            .into_iter()
            .any(|value| literal_value_matches(value, &context.typed));
    }

    if let Some(context) = variable_string_completion_context(&lexed.tokens, offset) {
        return match context.namespace {
            VariableNamespace::Environment => variables
                .environment_names()
                .any(|name| name.starts_with(&context.typed)),
            VariableNamespace::Variables => variables
                .variable_names()
                .any(|name| name.starts_with(&context.typed)),
        };
    }

    if lexed.ended_in_quoted_string {
        return false;
    }

    member_completion_context(&lexed.tokens, offset).is_some_and(|context| {
        specs_for_path(&context.path, phase)
            .iter()
            .any(|spec| spec.label.starts_with(&context.typed))
            || (catalog.is_some()
                && request_namespace_members_match(catalog.as_ref(), &context.path, &context.typed))
    })
}

/// Whether any exposed collection namespace member matches the typed prefix at
/// the given access path.
fn request_namespace_members_match(
    catalog: Option<&crate::core::RequestNamespaceCatalog>,
    path: &[String],
    typed: &str,
) -> bool {
    let Some(catalog) = catalog else {
        return false;
    };
    if path.is_empty() {
        return catalog.roots().any(|root| {
            root.status == crate::core::NodeStatus::Exposed && root.name.starts_with(typed)
        });
    }
    catalog.members_at(path).is_some_and(|members| {
        members.iter().any(|member| {
            member.status == crate::core::NodeStatus::Exposed && member.name.starts_with(typed)
        })
    })
}

/// Emits conservative warnings for missing literal variable reads.
///
/// Only the exact calls `api.environment.get("literal")` and
/// `api.variables.get("literal")` are checked. Comments, string contents,
/// dynamic arguments, `has`, and write calls are deliberately ignored. A
/// preceding literal `api.environment.set("name", value)` makes that name
/// available to later reads in the same source.
pub fn diagnostics_for_source(
    source: &str,
    variables: &ScriptVariableCatalog,
    catalog: Option<&crate::core::RequestNamespaceCatalog>,
) -> Vec<Diagnostic> {
    let lexed = lex(source);
    let tokens = &lexed.tokens;
    let mut script_environment_names = variables.environment_names.clone();
    let mut disabled_environment_names = variables.disabled_environment_names.clone();
    let mut diagnostics = Vec::new();

    let mut index = 0;
    while index < tokens.len() {
        let Some(call) = api_variable_call(tokens, index) else {
            index += 1;
            continue;
        };

        let Some(argument) = tokens.get(index + 6).and_then(Token::string_literal) else {
            index += 1;
            continue;
        };
        let Some(name) = argument.value.as_deref() else {
            index += 1;
            continue;
        };

        match (call.namespace, call.method) {
            (VariableNamespace::Environment, "set") => {
                script_environment_names.insert(name.to_owned());
                disabled_environment_names.remove(name);
            }
            (VariableNamespace::Environment, "get") if !script_environment_names.contains(name) => {
                diagnostics.push(variable_diagnostic(
                    source,
                    argument.span.clone(),
                    name,
                    disabled_environment_names.contains(name),
                    "active environment",
                ));
            }
            (VariableNamespace::Variables, "get")
                if !script_environment_names.contains(name)
                    && !variables.collection_names.contains(name) =>
            {
                diagnostics.push(variable_diagnostic(
                    source,
                    argument.span.clone(),
                    name,
                    disabled_environment_names.contains(name),
                    "active environment or collection",
                ));
            }
            _ => {}
        }

        index += 1;
    }

    // Diagnose stale / renamed saved-request references passed to
    // api.requests.execute(...) that no longer resolve in the active
    // workspace's collection tree.
    if let Some(catalog) = catalog {
        let mut index = 0;
        while index < tokens.len() {
            if let Some((span, path)) = api_requests_execute_path(tokens, index) {
                if path.len() >= 2 && catalog.request_at(&path).is_none() {
                    diagnostics.push(Diagnostic {
                        range: source_range(source, span.start, span.end),
                        severity: Some(DiagnosticSeverity::ERROR),
                        code: Some(NumberOrString::String("missing-request-reference".to_owned())),
                        source: Some(DIAGNOSTIC_SOURCE.to_owned()),
                        message: format!(
                            "Saved request '{}' is no longer available in the active workspace; it may have been renamed or moved.",
                            path.join(".")
                        ),
                        ..Default::default()
                    });
                }
            }
            index += 1;
        }
    }

    diagnostics
}

/// If `tokens[index..]` opens `api.requests.execute(` followed by a dotted
/// identifier path, returns the final identifier's span and the path segments.
fn api_requests_execute_path(
    tokens: &[Token],
    index: usize,
) -> Option<(std::ops::Range<usize>, Vec<String>)> {
    const KEY: &[&str] = &["api", "requests", "execute"];
    for (offset, expected) in KEY.iter().enumerate() {
        let position = index + (offset * 2);
        if tokens.get(position)?.identifier()? != *expected {
            return None;
        }
        if offset < 2 && !matches!(tokens.get(position + 1)?.kind, TokenKind::Dot) {
            return None;
        }
    }
    if !matches!(tokens.get(index + 5)?.kind, TokenKind::LeftParen) {
        return None;
    }
    let walk_start = index + 6;
    let mut path = Vec::new();
    let mut cursor = walk_start;
    let first = tokens.get(cursor)?.identifier()?;
    path.push(first.to_owned());
    let mut span = tokens[cursor].span.clone();
    cursor += 1;
    while matches!(tokens.get(cursor)?.kind, TokenKind::Dot) {
        let identifier = tokens.get(cursor + 1)?.identifier()?;
        span = tokens[cursor + 1].span.clone();
        path.push(identifier.to_owned());
        cursor += 2;
    }
    if path.len() < 2 {
        return None;
    }
    Some((span, path))
}

#[derive(Clone, Copy)]
struct CompletionSpec {
    label: &'static str,
    kind: CompletionItemKind,
    detail: &'static str,
    documentation: &'static str,
}

const ROOT_MEMBERS_PRE: &[CompletionSpec] = &[
    field(
        "request",
        "Request",
        "The request being prepared. Changes made here are used for the outgoing request.",
    ),
    field(
        "environment",
        "EnvironmentVariables",
        "Read and transactionally update variables from the selected environment.",
    ),
    field(
        "variables",
        "Variables",
        "Read variables from the selected environment with collection-variable fallback.",
    ),
    field(
        "requests",
        "RequestReferences",
        "Schedule saved requests from the active workspace's collection tree for execution after this script phase. This is distinct from fetch().",
    ),
    field("console", "ScriptConsole", "Bounded script console."),
];

const ROOT_MEMBERS_POST: &[CompletionSpec] = &[
    field(
        "request",
        "ReadonlyRequest",
        "The request that was sent. Request fields and headers are read-only in this phase.",
    ),
    field(
        "response",
        "Response",
        "The received response and its script-visible body.",
    ),
    field(
        "environment",
        "EnvironmentVariables",
        "Read and transactionally update variables from the selected environment.",
    ),
    field(
        "variables",
        "Variables",
        "Read variables from the selected environment with collection-variable fallback.",
    ),
    field(
        "requests",
        "RequestReferences",
        "Schedule saved requests from the active workspace's collection tree for execution after this script phase. This is distinct from fetch().",
    ),
    field("console", "ScriptConsole", "Bounded script console."),
    method(
        "test",
        "(name: unknown, callback: () => unknown): void",
        "Runs `callback` synchronously and records a passing or failing test named by `name`. Callback errors are recorded instead of aborting the script; async callbacks are unsupported and are recorded as failures.",
    ),
    method(
        "assert",
        "(condition: unknown, message?: unknown): void",
        "Throws when `condition` is falsy. `message` is converted to text and defaults to `\"assertion failed\"`.",
    ),
];

const REQUESTS_MEMBERS: &[CompletionSpec] = &[method(
    "execute",
    "(requestRef: SavedRequestReference): Promise<void>",
    "Runs a saved request from the active workspace. Await it to run the full pipeline (its own pre/post scripts, the HTTP exchange, and anything it chains) before the script continues and apply its environment mutations to the live environment; without await it runs after the current phase. Pass a request reference such as ChatAdmin.Login (never a string path); recursion is bounded and cycles are rejected.",
)];

const REQUEST_MEMBERS_PRE: &[CompletionSpec] = &[
    field(
        "method",
        "string",
        "HTTP method. Assignments are converted to text before sending.",
    ),
    field(
        "url",
        "string",
        "Request URL. Assignments are converted to text before sending.",
    ),
    field(
        "body",
        "string",
        "Raw request body. Assignments are converted to text before sending.",
    ),
    field(
        "bodyMode",
        "BodyMode",
        "Request body mode: `none`, `raw`, `form_url_encoded`, or `multipart_form_data`.",
    ),
    field(
        "rawBodyLanguage",
        "RawBodyLanguage",
        "Syntax language selected for a raw body. Use one of the values offered by completion.",
    ),
    field(
        "bodyFields",
        "BodyField[]",
        "Mutable form or multipart rows. A file row's `value` is its file path.",
    ),
    field(
        "headers",
        "MutableHeaders",
        "Mutable request header bag. Prefer its methods when editing headers.",
    ),
];

const REQUEST_MEMBERS_POST: &[CompletionSpec] = &[
    field("method", "string", "HTTP method that was sent. Read-only."),
    field("url", "string", "Request URL that was sent. Read-only."),
    field(
        "body",
        "string",
        "Raw request body that was sent. Read-only.",
    ),
    field(
        "bodyMode",
        "BodyMode",
        "Body mode that was sent. Read-only.",
    ),
    field(
        "rawBodyLanguage",
        "RawBodyLanguage",
        "Raw-body syntax language that was selected. Read-only.",
    ),
    field(
        "bodyFields",
        "readonly Readonly<BodyField>[]",
        "Read-only form or multipart rows from the sent request. A file row's `value` is its file path.",
    ),
    field(
        "headers",
        "ReadonlyHeaders",
        "Read-only header bag from the sent request.",
    ),
];

const RESPONSE_MEMBERS: &[CompletionSpec] = &[
    field("status", "number", "HTTP response status code."),
    field("statusText", "string", "HTTP response reason phrase."),
    field("httpVersion", "string", "Negotiated HTTP version."),
    field("url", "string", "Final response URL after redirects."),
    field(
        "headers",
        "ReadonlyHeaders",
        "Read-only response header bag.",
    ),
    field(
        "durationMs",
        "number",
        "Whole milliseconds elapsed while performing the request.",
    ),
    field(
        "sizeBytes",
        "number",
        "Full received response-body size in bytes, even when the script-visible body is truncated.",
    ),
    field(
        "truncated",
        "boolean",
        "Whether the script-visible response body was capped at 5 MiB.",
    ),
    field(
        "bodyBase64",
        "string | null",
        "Base64 of the script-visible body when its bytes were not valid UTF-8; otherwise `null`. The visible body can be truncated to 5 MiB.",
    ),
    method(
        "text",
        "(): string",
        "Returns the script-visible response body as text using lossy UTF-8 decoding. The visible body can be truncated to 5 MiB.",
    ),
    method(
        "json",
        "(): any",
        "Parses `text()` with `JSON.parse`. Invalid or truncated JSON throws.",
    ),
];

const READONLY_HEADER_MEMBERS: &[CompletionSpec] = &[
    method(
        "has",
        "(name: unknown): boolean",
        "Checks for an enabled header. Names are converted to text, trimmed, and compared case-insensitively.",
    ),
    method(
        "get",
        "(name: unknown): string | undefined",
        "Returns the first enabled matching header value. Names are converted to text, trimmed, and compared case-insensitively.",
    ),
    method(
        "getAll",
        "(name: unknown): string[]",
        "Returns all enabled matching header values. Names are converted to text, trimmed, and compared case-insensitively.",
    ),
    method(
        "toArray",
        "(): Header[]",
        "Returns detached header-row copies with `enabled`, `name`, and `value` fields.",
    ),
];

const MUTABLE_HEADER_MEMBERS: &[CompletionSpec] = &[
    method(
        "has",
        "(name: unknown): boolean",
        "Checks for an enabled header. Names are converted to text, trimmed, and compared case-insensitively.",
    ),
    method(
        "get",
        "(name: unknown): string | undefined",
        "Returns the first enabled matching header value. Names are converted to text, trimmed, and compared case-insensitively.",
    ),
    method(
        "getAll",
        "(name: unknown): string[]",
        "Returns all enabled matching header values. Names are converted to text, trimmed, and compared case-insensitively.",
    ),
    method(
        "set",
        "(name: unknown, value: unknown): void",
        "Sets one enabled request-header value, removing later enabled duplicates. The trimmed name cannot be empty; both arguments are converted to text.",
    ),
    method(
        "append",
        "(name: unknown, value: unknown): void",
        "Appends an enabled request header. The trimmed name cannot be empty; both arguments are converted to text.",
    ),
    method(
        "remove",
        "(name: unknown): void",
        "Removes all request headers with this case-insensitive name.",
    ),
    method(
        "toArray",
        "(): Header[]",
        "Returns detached header-row copies with `enabled`, `name`, and `value` fields.",
    ),
];

const ENVIRONMENT_MEMBERS: &[CompletionSpec] = &[
    method(
        "has",
        "(key: unknown): boolean",
        "Checks the selected environment for an exact, case-sensitive key after converting `key` to text.",
    ),
    method(
        "get",
        "(key: unknown): string | undefined",
        "Reads an exact, case-sensitive key from the selected environment after converting `key` to text.",
    ),
    method(
        "set",
        "(key: unknown, value: unknown): void",
        "Updates the current script's environment view and queues persistence only if the script succeeds. The non-empty key and value are converted to text.",
    ),
    method(
        "unset",
        "(key: unknown): void",
        "Removes the key from the current script's environment view and queues persistence only if the script succeeds.",
    ),
    method(
        "toObject",
        "(): Record<string, string>",
        "Returns a detached object containing the selected environment's variables.",
    ),
];

const VARIABLE_MEMBERS: &[CompletionSpec] = &[
    method(
        "has",
        "(key: unknown): boolean",
        "Checks the selected environment and then collection variables for an exact, case-sensitive key. Environment variables take precedence.",
    ),
    method(
        "get",
        "(key: unknown): string | undefined",
        "Reads an exact, case-sensitive key from the selected environment, then falls back to collection variables.",
    ),
    method(
        "toObject",
        "(): Record<string, string>",
        "Returns a detached merged object. Selected-environment variables override same-named collection variables.",
    ),
];

const CONSOLE_MEMBERS: &[CompletionSpec] = &[
    method(
        "log",
        "(...values: unknown[]): void",
        "Writes one `log` row. Values are rendered and space-joined; all script logs together are capped at 100 rows and 64 KiB.",
    ),
    method(
        "info",
        "(...values: unknown[]): void",
        "Writes one `info` row. Values are rendered and space-joined; all script logs together are capped at 100 rows and 64 KiB.",
    ),
    method(
        "warn",
        "(...values: unknown[]): void",
        "Writes one `warn` row. Values are rendered and space-joined; all script logs together are capped at 100 rows and 64 KiB.",
    ),
    method(
        "error",
        "(...values: unknown[]): void",
        "Writes one `error` row. Values are rendered and space-joined; all script logs together are capped at 100 rows and 64 KiB.",
    ),
    method(
        "debug",
        "(...values: unknown[]): void",
        "Writes one `debug` row. Values are rendered and space-joined; all script logs together are capped at 100 rows and 64 KiB.",
    ),
];

const GLOBALS_PRE: &[CompletionSpec] = &[
    field(
        "api",
        "PreRequestApi",
        "Sandboxed synchronous Resolved runtime. Network and host globals such as `fetch`, `require`, `process`, `Deno`, `WebSocket`, and `XMLHttpRequest` are unavailable. Each run is limited to 1 second, a 32 MiB heap, and a 256 KiB stack.",
    ),
    field(
        "console",
        "ScriptConsole",
        "Global alias of `api.console`, writing bounded rows to the script log.",
    ),
];

const GLOBALS_POST: &[CompletionSpec] = &[
    field(
        "api",
        "PostResponseApi",
        "Sandboxed synchronous Resolved runtime. Network and host globals such as `fetch`, `require`, `process`, `Deno`, `WebSocket`, and `XMLHttpRequest` are unavailable. Each run is limited to 1 second, a 32 MiB heap, and a 256 KiB stack.",
    ),
    field(
        "console",
        "ScriptConsole",
        "Global alias of `api.console`, writing bounded rows to the script log.",
    ),
];

const fn field(
    label: &'static str,
    detail: &'static str,
    documentation: &'static str,
) -> CompletionSpec {
    CompletionSpec {
        label,
        kind: CompletionItemKind::FIELD,
        detail,
        documentation,
    }
}

const fn method(
    label: &'static str,
    detail: &'static str,
    documentation: &'static str,
) -> CompletionSpec {
    CompletionSpec {
        label,
        kind: CompletionItemKind::METHOD,
        detail,
        documentation,
    }
}

fn completion_items(
    source: &str,
    requested_offset: usize,
    phase: ScriptEditorPhase,
    variables: &ScriptVariableCatalog,
    expose_environment_values: bool,
    catalog: Option<&crate::core::RequestNamespaceCatalog>,
) -> Vec<CompletionItem> {
    let offset = clipped_char_boundary(source, requested_offset);
    let prefix = &source[..offset];
    let lexed = lex(prefix);

    if lexed.ended_in_comment_or_template {
        return Vec::new();
    }

    if let Some(string_context) =
        request_literal_completion_context(prefix, &lexed.tokens, offset, phase)
    {
        let replace_end = quoted_content_end(
            source,
            string_context.replace_start,
            offset,
            string_context.quote,
        );
        return string_context
            .kind
            .values()
            .into_iter()
            .filter(|value| literal_value_matches(value, &string_context.typed))
            .map(|value| {
                request_literal_completion_item(
                    source,
                    string_context.replace_start,
                    replace_end,
                    value,
                    string_context.quote,
                    string_context.kind,
                )
            })
            .collect();
    }

    if let Some(string_context) = variable_string_completion_context(&lexed.tokens, offset) {
        let (environment_names, _) =
            script_environment_state_before(&lexed.tokens, string_context.call_start, variables);
        let item_context = VariableCompletionItemContext {
            source,
            replace_start: string_context.replace_start,
            offset,
            quote: string_context.quote,
            namespace: string_context.namespace,
            environment_names: &environment_names,
            environment_values: &variables.environment_values,
            expose_environment_values,
        };
        let names: Box<dyn Iterator<Item = &str> + '_> = match string_context.namespace {
            VariableNamespace::Environment => {
                Box::new(environment_names.iter().map(String::as_str))
            }
            VariableNamespace::Variables => Box::new(
                environment_names
                    .union(&variables.collection_names)
                    .map(String::as_str),
            ),
        };
        return names
            .filter(|name| name.starts_with(&string_context.typed))
            .map(|name| variable_completion_item(name, &item_context))
            .collect();
    }

    if lexed.ended_in_quoted_string {
        return Vec::new();
    }

    let Some(context) = member_completion_context(&lexed.tokens, offset) else {
        return Vec::new();
    };
    let specs = specs_for_path(&context.path, phase);
    let mut items: Vec<CompletionItem> = specs
        .iter()
        .filter(|spec| spec.label.starts_with(&context.typed))
        .map(|spec| spec_completion_item(source, context.replace_start, offset, *spec))
        .collect();

    if let Some(catalog) = catalog {
        items.extend(request_namespace_items(
            catalog,
            &context.path,
            &context.typed,
            source,
            context.replace_start,
            offset,
        ));
    }

    items
}

/// Completion items contributed by the active workspace's saved-request
/// collection namespace (root collections, folders, and request references).
/// This is the editor half of the single catalog model the runtime injects.
fn request_namespace_items(
    catalog: &crate::core::RequestNamespaceCatalog,
    path: &[String],
    typed: &str,
    source: &str,
    replace_start: usize,
    offset: usize,
) -> Vec<CompletionItem> {
    let mut items = Vec::new();
    if path.is_empty() {
        // Root-level collection namespaces participate in completion.
        for root in catalog.roots() {
            if root.status != crate::core::NodeStatus::Exposed {
                continue;
            }
            if !root.name.starts_with(typed) {
                continue;
            }
            let Some(new_text) = dot_or_bracket_text(&root.access) else {
                continue;
            };
            items.push(request_namespace_item(
                source,
                replace_start,
                offset,
                &root.name,
                new_text,
                "collection · saved-request namespace",
                "A saved-request collection from the active workspace. Type the collection name, then `.`, to reach its folders and requests.",
            ));
        }
        return items;
    }

    let Some(members) = catalog.members_at(path) else {
        return items;
    };
    for member in members {
        if member.status != crate::core::NodeStatus::Exposed {
            continue;
        }
        if !member.name.starts_with(typed) {
            continue;
        }
        let Some(new_text) = dot_or_bracket_text(&member.access) else {
            continue;
        };
        let (detail, documentation) = match &member.kind {
            crate::core::NodeKind::Namespace => (
                "collection folder".to_owned(),
                "A saved-request folder. Type the folder name, then `.`, to reach its requests.".to_owned(),
            ),
            crate::core::NodeKind::Request(info) => (
                format!("{} · {} · Saved request", info.method, info.url_template),
                "A saved request. Pass it to api.requests.execute(...) to run it through the normal request pipeline.".to_owned(),
            ),
        };
        items.push(request_namespace_item(
            source,
            replace_start,
            offset,
            &member.name,
            new_text,
            &detail,
            &documentation,
        ));
    }
    items
}

/// For a dot-valid identifier use the plain name; otherwise emit bracket
/// notation so the inserted text remains valid JavaScript the runtime resolves.
fn dot_or_bracket_text(access: &crate::core::AccessStep) -> Option<String> {
    match access {
        crate::core::AccessStep::Dot(name) => Some(name.clone()),
        crate::core::AccessStep::Bracket(name) => {
            let escaped = name.replace('\\', "\\\\").replace('"', "\\\"");
            Some(format!("[\"{escaped}\"]"))
        }
    }
}

fn request_namespace_item(
    source: &str,
    replace_start: usize,
    offset: usize,
    name: &str,
    new_text: String,
    detail: &str,
    documentation: &str,
) -> CompletionItem {
    CompletionItem {
        label: name.to_owned(),
        kind: Some(CompletionItemKind::FIELD),
        detail: Some(detail.to_owned()),
        documentation: Some(Documentation::String(documentation.to_owned())),
        text_edit: Some(CompletionTextEdit::Edit(TextEdit {
            range: source_range(source, replace_start, offset),
            new_text,
        })),
        ..Default::default()
    }
}

fn specs_for_path(path: &[String], phase: ScriptEditorPhase) -> &'static [CompletionSpec] {
    match path {
        [] => match phase {
            ScriptEditorPhase::PreRequest => GLOBALS_PRE,
            ScriptEditorPhase::PostResponse => GLOBALS_POST,
        },
        [api] if api == "api" => match phase {
            ScriptEditorPhase::PreRequest => ROOT_MEMBERS_PRE,
            ScriptEditorPhase::PostResponse => ROOT_MEMBERS_POST,
        },
        [api, request] if api == "api" && request == "request" => match phase {
            ScriptEditorPhase::PreRequest => REQUEST_MEMBERS_PRE,
            ScriptEditorPhase::PostResponse => REQUEST_MEMBERS_POST,
        },
        [api, request, headers] if api == "api" && request == "request" && headers == "headers" => {
            match phase {
                ScriptEditorPhase::PreRequest => MUTABLE_HEADER_MEMBERS,
                ScriptEditorPhase::PostResponse => READONLY_HEADER_MEMBERS,
            }
        }
        [api, response] if api == "api" && response == "response" => {
            if phase == ScriptEditorPhase::PostResponse {
                RESPONSE_MEMBERS
            } else {
                &[]
            }
        }
        [api, response, headers]
            if api == "api" && response == "response" && headers == "headers" =>
        {
            if phase == ScriptEditorPhase::PostResponse {
                READONLY_HEADER_MEMBERS
            } else {
                &[]
            }
        }
        [api, environment] if api == "api" && environment == "environment" => ENVIRONMENT_MEMBERS,
        [api, variables] if api == "api" && variables == "variables" => VARIABLE_MEMBERS,
        [api, requests] if api == "api" && requests == "requests" => REQUESTS_MEMBERS,
        [api, console] if api == "api" && console == "console" => CONSOLE_MEMBERS,
        [console] if console == "console" => CONSOLE_MEMBERS,
        _ => &[],
    }
}

fn hover_for_source(
    source: &str,
    requested_offset: usize,
    phase: ScriptEditorPhase,
    variables: &ScriptVariableCatalog,
    expose_environment_values: bool,
    catalog: Option<&crate::core::RequestNamespaceCatalog>,
) -> Option<Hover> {
    let offset = clipped_char_boundary(source, requested_offset);
    let tokens = lex(source).tokens;
    let token_index = hover_token_at_offset(&tokens, offset)?;

    if tokens[token_index].identifier().is_some() {
        return runtime_symbol_hover(source, &tokens, token_index, phase, catalog);
    }
    if tokens[token_index].string_literal().is_some() {
        return variable_name_hover(
            source,
            &tokens,
            token_index,
            offset,
            phase,
            variables,
            expose_environment_values,
        );
    }
    None
}

fn hover_token_at_offset(tokens: &[Token], offset: usize) -> Option<usize> {
    let containing = tokens
        .iter()
        .position(|token| token.span.start <= offset && offset < token.span.end);
    if let Some(index) = containing
        && (tokens[index].identifier().is_some() || tokens[index].string_literal().is_some())
    {
        return Some(index);
    }

    // GPUI hit-testing returns the closest caret position rather than the
    // painted glyph itself. On the right half of an identifier's final glyph,
    // that position is the identifier end (often also the next punctuation
    // token's start). Resolve that boundary to the identifier on its left and
    // let the popover retain that trigger offset while keeping its semantic
    // range scoped to the identifier.
    tokens
        .iter()
        .rposition(|token| token.identifier().is_some() && token.span.end == offset)
}

fn runtime_symbol_hover(
    source: &str,
    tokens: &[Token],
    token_index: usize,
    phase: ScriptEditorPhase,
    catalog: Option<&crate::core::RequestNamespaceCatalog>,
) -> Option<Hover> {
    let symbol = tokens.get(token_index)?;
    let symbol_name = symbol.identifier()?;
    let path = dotted_identifier_path(tokens.get(..=token_index)?)?;
    let (resolved_name, parent_path) = path.split_last()?;
    if resolved_name != symbol_name {
        return None;
    }
    let full_path = path.join(".");
    // A frozen saved-request reference from the active workspace's collection
    // tree gets its own hover identifying it as such, with only non-secret
    // metadata.
    if let Some(catalog) = catalog {
        if let Some(info) = catalog.request_at(&path) {
            return Some(markdown_hover(
                source,
                symbol.span.start,
                symbol.span.end,
                request_reference_hover(&full_path, info, phase),
            ));
        }
    }
    let spec = specs_for_path(parent_path, phase)
        .iter()
        .find(|spec| spec.label == symbol_name)?;
    Some(markdown_hover(
        source,
        symbol.span.start,
        symbol.span.end,
        runtime_symbol_markdown(&full_path, *spec, phase),
    ))
}

/// Markdown hover for a frozen saved-request reference. Only non-secret
/// metadata is shown (name, collection path, method, template URL).
fn request_reference_hover(
    full_path: &str,
    info: &crate::core::RequestRefInfo,
    phase: ScriptEditorPhase,
) -> String {
    format!(
        "```typescript\n{}\n```\n\n**Saved request reference** · `{}`\n\n- Collection path: `{}`\n- Method: `{}`\n- Template URL: `{}`\n\nPass it to `api.requests.execute(...)` to run it through the normal request pipeline.\n\n_{}_",
        full_path,
        full_path,
        info.collection_path.join("."),
        info.method,
        info.url_template,
        phase_availability(phase),
    )
}

fn variable_name_hover(
    source: &str,
    tokens: &[Token],
    token_index: usize,
    offset: usize,
    phase: ScriptEditorPhase,
    variables: &ScriptVariableCatalog,
    expose_environment_values: bool,
) -> Option<Hover> {
    let argument = tokens.get(token_index)?.string_literal()?;
    let name = argument.value.as_deref()?;
    let call_start = token_index.checked_sub(6)?;
    let call = api_variable_call(tokens, call_start)?;
    let accepts_variable_name = match call.namespace {
        VariableNamespace::Environment => {
            matches!(call.method, "get" | "has" | "set" | "unset")
        }
        VariableNamespace::Variables => matches!(call.method, "get" | "has"),
    };
    if !accepts_variable_name {
        return None;
    }

    let parent_path = match call.namespace {
        VariableNamespace::Environment => vec!["api".to_owned(), "environment".to_owned()],
        VariableNamespace::Variables => vec!["api".to_owned(), "variables".to_owned()],
    };
    let method = specs_for_path(&parent_path, phase)
        .iter()
        .find(|spec| spec.label == call.method)?;
    let full_path = format!("{}.{}", parent_path.join("."), call.method);
    let content_end = if argument.terminated {
        argument.span.end.saturating_sub(argument.quote.len_utf8())
    } else {
        argument.span.end
    };
    if argument.content_start >= content_end {
        return None;
    }
    if !(argument.content_start <= offset && offset <= content_end) {
        return None;
    }
    let (environment_names, disabled_environment_names) =
        script_environment_state_before(tokens, call_start, variables);
    let variable_state = VariableHoverState {
        environment_names,
        environment_values: &variables.environment_values,
        disabled_environment_names,
        collection_names: &variables.collection_names,
    };

    Some(markdown_hover(
        source,
        argument.content_start,
        content_end,
        variable_name_markdown(
            name,
            call,
            &variable_state,
            &full_path,
            *method,
            phase,
            expose_environment_values,
        ),
    ))
}

fn runtime_symbol_markdown(
    full_path: &str,
    spec: CompletionSpec,
    phase: ScriptEditorPhase,
) -> String {
    format!(
        "```typescript\n{}\n```\n\n{}\n\n_{}_",
        runtime_symbol_signature(full_path, spec),
        spec.documentation,
        phase_availability(phase),
    )
}

struct VariableHoverState<'a> {
    environment_names: BTreeSet<String>,
    environment_values: &'a BTreeMap<String, String>,
    disabled_environment_names: BTreeSet<String>,
    collection_names: &'a BTreeSet<String>,
}

fn variable_name_markdown(
    name: &str,
    call: ApiVariableCall<'_>,
    variable_state: &VariableHoverState<'_>,
    full_path: &str,
    method: CompletionSpec,
    phase: ScriptEditorPhase,
    expose_environment_values: bool,
) -> String {
    let status = variable_name_status(name, call.namespace, variable_state);
    let resolves_from_selected_environment = match call.namespace {
        VariableNamespace::Environment => true,
        VariableNamespace::Variables => variable_state.environment_names.contains(name),
    };
    let selected_environment_value = (expose_environment_values
        && resolves_from_selected_environment)
        .then(|| variable_state.environment_values.get(name))
        .flatten()
        .map(|value| {
            format!(
                "\n\n**Selected environment value:** {}",
                markdown_inline_code(value)
            )
        })
        .unwrap_or_default();
    format!(
        "```typescript\n{}\n```\n\n{}\n\n**Variable key:** {}\n\n{}{}\n\n_{}_",
        runtime_symbol_signature(full_path, method),
        method.documentation,
        markdown_inline_code(name),
        status,
        selected_environment_value,
        phase_availability(phase),
    )
}

fn variable_name_status(
    name: &str,
    namespace: VariableNamespace,
    variable_state: &VariableHoverState<'_>,
) -> &'static str {
    if variable_state.environment_names.contains(name) {
        return "Available in the selected environment.";
    }
    if namespace == VariableNamespace::Variables && variable_state.collection_names.contains(name) {
        return "Available as a collection variable. No selected-environment variable shadows it.";
    }
    if variable_state.disabled_environment_names.contains(name) {
        return "This key exists in the selected environment but is disabled.";
    }
    match namespace {
        VariableNamespace::Environment => {
            "This key does not currently exist in the selected environment."
        }
        VariableNamespace::Variables => {
            "This key does not currently exist in the selected environment or collection."
        }
    }
}

fn script_environment_state_before(
    tokens: &[Token],
    end_index: usize,
    variables: &ScriptVariableCatalog,
) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut environment_names = variables.environment_names.clone();
    let mut disabled_environment_names = variables.disabled_environment_names.clone();
    let mut index = 0;

    while index < end_index {
        let Some(call) = api_variable_call(tokens, index) else {
            index += 1;
            continue;
        };
        if call.namespace != VariableNamespace::Environment {
            index += 1;
            continue;
        }
        let Some(name) = tokens
            .get(index + 6)
            .filter(|_| index + 6 < end_index)
            .and_then(Token::string_literal)
            .and_then(|argument| argument.value.as_deref())
        else {
            index += 1;
            continue;
        };

        match call.method {
            "set" if !name.is_empty() => {
                environment_names.insert(name.to_owned());
                disabled_environment_names.remove(name);
            }
            "unset" => {
                environment_names.remove(name);
                disabled_environment_names.remove(name);
            }
            _ => {}
        }
        index += 1;
    }

    (environment_names, disabled_environment_names)
}

fn runtime_symbol_signature(full_path: &str, spec: CompletionSpec) -> String {
    if spec.kind == CompletionItemKind::METHOD {
        format!("{full_path}{}", spec.detail)
    } else {
        format!("{full_path}: {}", spec.detail)
    }
}

fn phase_availability(phase: ScriptEditorPhase) -> &'static str {
    match phase {
        ScriptEditorPhase::PreRequest => "Available in pre-request scripts.",
        ScriptEditorPhase::PostResponse => "Available in post-response scripts.",
    }
}

fn markdown_inline_code(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            character if character.is_control() => {
                use std::fmt::Write as _;
                let _ = write!(escaped, "\\u{{{:x}}}", character as u32);
            }
            character => escaped.push(character),
        }
    }
    let fence_length = escaped
        .as_bytes()
        .split(|byte| *byte != b'`')
        .map(<[u8]>::len)
        .max()
        .unwrap_or_default()
        .saturating_add(1)
        .max(1);
    let fence = "`".repeat(fence_length);
    format!("{fence} {escaped} {fence}")
}

fn markdown_hover(source: &str, start: usize, end: usize, value: String) -> Hover {
    Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value,
        }),
        range: Some(source_range(source, start, end)),
    }
}

fn spec_completion_item(
    source: &str,
    replace_start: usize,
    offset: usize,
    spec: CompletionSpec,
) -> CompletionItem {
    CompletionItem {
        label: spec.label.to_owned(),
        kind: Some(spec.kind),
        detail: Some(spec.detail.to_owned()),
        documentation: Some(Documentation::String(spec.documentation.to_owned())),
        text_edit: Some(CompletionTextEdit::Edit(TextEdit {
            range: source_range(source, replace_start, offset),
            new_text: spec.label.to_owned(),
        })),
        ..Default::default()
    }
}

struct VariableCompletionItemContext<'a> {
    source: &'a str,
    replace_start: usize,
    offset: usize,
    quote: char,
    namespace: VariableNamespace,
    environment_names: &'a BTreeSet<String>,
    environment_values: &'a BTreeMap<String, String>,
    expose_environment_values: bool,
}

fn variable_completion_item(
    name: &str,
    context: &VariableCompletionItemContext<'_>,
) -> CompletionItem {
    let detail = match context.namespace {
        VariableNamespace::Environment => "selected-environment variable",
        VariableNamespace::Variables if context.environment_names.contains(name) => {
            "selected-environment variable"
        }
        VariableNamespace::Variables => "collection variable",
    };
    let resolves_from_selected_environment = match context.namespace {
        VariableNamespace::Environment => true,
        VariableNamespace::Variables => context.environment_names.contains(name),
    };
    let documentation =
        if context.expose_environment_values && resolves_from_selected_environment {
            context.environment_values.get(name).map(|value| {
                Documentation::MarkupContent(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: format!(
                        "**Selected environment value:** {}",
                        markdown_inline_code(value)
                    ),
                })
            })
        } else {
            None
        }
        .unwrap_or_else(|| {
            Documentation::String(
                match detail {
                    "selected-environment variable" => "Available in the selected environment.",
                    _ => "Available as a collection variable.",
                }
                .to_owned(),
            )
        });
    CompletionItem {
        label: name.to_owned(),
        kind: Some(CompletionItemKind::VARIABLE),
        detail: Some(detail.to_owned()),
        documentation: Some(documentation),
        text_edit: Some(CompletionTextEdit::Edit(TextEdit {
            range: source_range(context.source, context.replace_start, context.offset),
            new_text: escape_for_quote(name, context.quote),
        })),
        ..Default::default()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RequestLiteralKind {
    Method,
    BodyMode,
    RawBodyLanguage,
    BodyFieldKind,
}

impl RequestLiteralKind {
    fn values(self) -> Vec<&'static str> {
        match self {
            Self::Method => STANDARD_HTTP_METHODS.to_vec(),
            Self::BodyMode => BodyMode::all()
                .iter()
                .map(|mode| mode.as_db_str())
                .collect(),
            Self::RawBodyLanguage => RawBodyLanguage::all()
                .iter()
                .map(|language| language.as_db_str())
                .collect(),
            Self::BodyFieldKind => BodyFieldKind::all()
                .iter()
                .map(|kind| kind.as_db_str())
                .collect(),
        }
    }

    const fn detail(self) -> &'static str {
        match self {
            Self::Method => "common HTTP method",
            Self::BodyMode => "request body mode",
            Self::RawBodyLanguage => "raw body language",
            Self::BodyFieldKind => "body field kind",
        }
    }

    const fn documentation(self) -> &'static str {
        match self {
            Self::Method => {
                "Common HTTP method. The request editor still accepts custom extension methods."
            }
            Self::BodyMode => "Canonical body mode accepted by the script runtime.",
            Self::RawBodyLanguage => {
                "Canonical raw-body syntax language accepted by the script runtime."
            }
            Self::BodyFieldKind => {
                "Canonical structured-body field kind accepted by the script runtime."
            }
        }
    }
}

fn request_literal_completion_item(
    source: &str,
    replace_start: usize,
    replace_end: usize,
    value: &str,
    quote: char,
    kind: RequestLiteralKind,
) -> CompletionItem {
    CompletionItem {
        label: value.to_owned(),
        kind: Some(CompletionItemKind::ENUM_MEMBER),
        detail: Some(kind.detail().to_owned()),
        documentation: Some(Documentation::String(kind.documentation().to_owned())),
        text_edit: Some(CompletionTextEdit::Edit(TextEdit {
            range: source_range(source, replace_start, replace_end),
            new_text: escape_for_quote(value, quote),
        })),
        ..Default::default()
    }
}

fn quoted_content_end(
    source: &str,
    content_start: usize,
    cursor_offset: usize,
    quote: char,
) -> usize {
    let Some(literal_start) = content_start.checked_sub(quote.len_utf8()) else {
        return cursor_offset;
    };
    let literal = parse_string_literal(source, literal_start, quote);
    if literal.content_start != content_start {
        return cursor_offset;
    }

    if literal.terminated {
        literal.span.end.saturating_sub(quote.len_utf8())
    } else {
        literal.span.end.max(cursor_offset)
    }
}

fn literal_value_matches(value: &str, typed: &str) -> bool {
    value
        .get(..typed.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(typed))
}

fn escape_for_quote(name: &str, quote: char) -> String {
    let mut escaped = String::with_capacity(name.len());
    for character in name.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            value if value == quote => {
                escaped.push('\\');
                escaped.push(value);
            }
            value => escaped.push(value),
        }
    }
    escaped
}

#[derive(Debug)]
struct MemberCompletionContext {
    path: Vec<String>,
    typed: String,
    replace_start: usize,
}

fn member_completion_context(tokens: &[Token], offset: usize) -> Option<MemberCompletionContext> {
    let (path_end, typed, replace_start) = match tokens.last() {
        Some(Token {
            kind: TokenKind::Identifier(identifier),
            span,
        }) if span.end == offset => {
            if tokens.len() >= 2 && matches!(tokens[tokens.len() - 2].kind, TokenKind::Dot) {
                (tokens.len() - 2, identifier.clone(), span.start)
            } else {
                return Some(MemberCompletionContext {
                    path: Vec::new(),
                    typed: identifier.clone(),
                    replace_start: span.start,
                });
            }
        }
        Some(Token {
            kind: TokenKind::Dot,
            span,
        }) if span.end == offset => (tokens.len() - 1, String::new(), offset),
        None => {
            return Some(MemberCompletionContext {
                path: Vec::new(),
                typed: String::new(),
                replace_start: offset,
            });
        }
        _ => return None,
    };

    let path = dotted_identifier_path(&tokens[..path_end])?;
    Some(MemberCompletionContext {
        path,
        typed,
        replace_start,
    })
}

fn dotted_identifier_path(tokens: &[Token]) -> Option<Vec<String>> {
    if tokens.is_empty() {
        return None;
    }

    let mut reversed = Vec::new();
    let mut cursor = tokens.len();
    loop {
        let identifier = tokens.get(cursor.checked_sub(1)?)?.identifier()?;
        reversed.push(identifier.to_owned());
        cursor -= 1;

        if cursor < 2 || !matches!(tokens[cursor - 1].kind, TokenKind::Dot) {
            break;
        }
        cursor -= 1;
    }

    // A leading dot means this was a suffix of another expression rather than
    // the global `api`/`console` path.
    if cursor > 0 && matches!(tokens[cursor - 1].kind, TokenKind::Dot) {
        return None;
    }

    reversed.reverse();
    Some(reversed)
}

#[derive(Debug)]
struct RequestLiteralCompletionContext {
    kind: RequestLiteralKind,
    typed: String,
    replace_start: usize,
    quote: char,
}

fn request_literal_completion_context(
    source: &str,
    tokens: &[Token],
    offset: usize,
    phase: ScriptEditorPhase,
) -> Option<RequestLiteralCompletionContext> {
    if phase != ScriptEditorPhase::PreRequest {
        return None;
    }

    let argument = tokens.last()?.string_literal()?;
    if argument.terminated || argument.span.end != offset {
        return None;
    }

    let separator_index = tokens.len().checked_sub(2)?;
    let separator = tokens.get(separator_index)?;
    let target_tokens = &tokens[..separator_index];
    let kind = if token_source_is(source, separator, "=") {
        request_assignment_literal_kind(source, target_tokens)
    } else if token_source_is(source, separator, ":") {
        body_field_object_literal_kind(source, target_tokens)
    } else {
        None
    }?;

    Some(RequestLiteralCompletionContext {
        kind,
        typed: argument.value.clone()?,
        replace_start: argument.content_start,
        quote: argument.quote,
    })
}

fn request_assignment_literal_kind(
    source: &str,
    target_tokens: &[Token],
) -> Option<RequestLiteralKind> {
    if let Some(path) = dotted_identifier_path(target_tokens) {
        return match path.as_slice() {
            [api, request, method]
                if api == "api" && request == "request" && method == "method" =>
            {
                Some(RequestLiteralKind::Method)
            }
            [api, request, body_mode]
                if api == "api" && request == "request" && body_mode == "bodyMode" =>
            {
                Some(RequestLiteralKind::BodyMode)
            }
            [api, request, language]
                if api == "api" && request == "request" && language == "rawBodyLanguage" =>
            {
                Some(RequestLiteralKind::RawBodyLanguage)
            }
            _ => None,
        };
    }

    body_field_kind_assignment_target(source, target_tokens)
        .then_some(RequestLiteralKind::BodyFieldKind)
}

fn body_field_kind_assignment_target(source: &str, tokens: &[Token]) -> bool {
    if tokens.len() < 7
        || tokens.last().and_then(Token::identifier) != Some("kind")
        || !matches!(
            tokens.get(tokens.len() - 2).map(|token| &token.kind),
            Some(TokenKind::Dot)
        )
    {
        return false;
    }

    let right_bracket_index = tokens.len() - 3;
    if !token_source_is(source, &tokens[right_bracket_index], "]") {
        return false;
    }

    let Some(left_bracket_index) = (0..right_bracket_index)
        .rev()
        .find(|index| token_source_is(source, &tokens[*index], "["))
    else {
        return false;
    };

    if tokens[left_bracket_index + 1..right_bracket_index]
        .iter()
        .any(|token| token_source_is(source, token, "[") || token_source_is(source, token, "]"))
    {
        return false;
    }

    let Some(index_text) = source
        .get(tokens[left_bracket_index].span.end..tokens[right_bracket_index].span.start)
        .map(str::trim)
    else {
        return false;
    };
    if !simple_body_field_index(index_text) {
        return false;
    }

    dotted_identifier_path(&tokens[..left_bracket_index]).is_some_and(|path| {
        matches!(
            path.as_slice(),
            [api, request, body_fields]
                if api == "api" && request == "request" && body_fields == "bodyFields"
        )
    })
}

fn simple_body_field_index(index: &str) -> bool {
    if index.is_empty() {
        return false;
    }
    if index.bytes().all(|byte| byte.is_ascii_digit()) {
        return true;
    }

    let mut characters = index.chars();
    characters.next().is_some_and(is_identifier_start) && characters.all(is_identifier_continue)
}

fn body_field_object_literal_kind(
    source: &str,
    target_tokens: &[Token],
) -> Option<RequestLiteralKind> {
    if target_tokens.last().and_then(Token::identifier) != Some("kind") {
        return None;
    }

    let mut nested_braces = 0usize;
    let mut object_start = None;
    for index in (0..target_tokens.len().saturating_sub(1)).rev() {
        let token = &target_tokens[index];
        if token_source_is(source, token, "}") {
            nested_braces = nested_braces.saturating_add(1);
        } else if token_source_is(source, token, "{") {
            if nested_braces == 0 {
                object_start = Some(index);
                break;
            }
            nested_braces -= 1;
        }
    }
    let object_start = object_start?;
    if object_start < 4
        || !matches!(
            target_tokens.get(object_start - 1).map(|token| &token.kind),
            Some(TokenKind::LeftParen)
        )
        || target_tokens
            .get(object_start - 2)
            .and_then(Token::identifier)
            != Some("push")
        || !matches!(
            target_tokens.get(object_start - 3).map(|token| &token.kind),
            Some(TokenKind::Dot)
        )
    {
        return None;
    }

    dotted_identifier_path(&target_tokens[..object_start - 3])
        .is_some_and(|path| {
            matches!(
                path.as_slice(),
                [api, request, body_fields]
                    if api == "api" && request == "request" && body_fields == "bodyFields"
            )
        })
        .then_some(RequestLiteralKind::BodyFieldKind)
}

fn token_source_is(source: &str, token: &Token, expected: &str) -> bool {
    source.get(token.span.clone()) == Some(expected)
}

#[derive(Debug)]
struct VariableStringCompletionContext {
    namespace: VariableNamespace,
    typed: String,
    replace_start: usize,
    quote: char,
    call_start: usize,
}

fn variable_string_completion_context(
    tokens: &[Token],
    offset: usize,
) -> Option<VariableStringCompletionContext> {
    let argument = tokens.last()?.string_literal()?;
    if argument.terminated || argument.span.end != offset {
        return None;
    }

    let call_start = tokens.len().checked_sub(7)?;
    let call = api_variable_call(tokens, call_start)?;
    let accepts_name_completion = match call.namespace {
        VariableNamespace::Environment => {
            matches!(call.method, "get" | "has" | "set" | "unset")
        }
        VariableNamespace::Variables => matches!(call.method, "get" | "has"),
    };
    if !accepts_name_completion {
        return None;
    }

    Some(VariableStringCompletionContext {
        namespace: call.namespace,
        typed: argument.value.clone()?,
        replace_start: argument.content_start,
        quote: argument.quote,
        call_start,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VariableNamespace {
    Environment,
    Variables,
}

#[derive(Clone, Copy, Debug)]
struct ApiVariableCall<'a> {
    namespace: VariableNamespace,
    method: &'a str,
}

fn api_variable_call<'a>(tokens: &'a [Token], start: usize) -> Option<ApiVariableCall<'a>> {
    if start > 0 && matches!(tokens[start - 1].kind, TokenKind::Dot) {
        return None;
    }
    if tokens.get(start)?.identifier()? != "api"
        || !matches!(tokens.get(start + 1)?.kind, TokenKind::Dot)
        || !matches!(tokens.get(start + 3)?.kind, TokenKind::Dot)
        || !matches!(tokens.get(start + 5)?.kind, TokenKind::LeftParen)
    {
        return None;
    }

    let namespace = match tokens.get(start + 2)?.identifier()? {
        "environment" => VariableNamespace::Environment,
        "variables" => VariableNamespace::Variables,
        _ => return None,
    };
    let method = tokens.get(start + 4)?.identifier()?;
    Some(ApiVariableCall { namespace, method })
}

fn variable_diagnostic(
    source: &str,
    span: std::ops::Range<usize>,
    name: &str,
    disabled: bool,
    scope: &str,
) -> Diagnostic {
    let (code, message) = if disabled {
        (
            DISABLED_VARIABLE_CODE,
            format!("Variable `{name}` is disabled in the active environment."),
        )
    } else {
        (
            MISSING_VARIABLE_CODE,
            format!("Variable `{name}` does not exist in the {scope}."),
        )
    };
    Diagnostic {
        range: source_range(source, span.start, span.end),
        severity: Some(DiagnosticSeverity::WARNING),
        code: Some(NumberOrString::String(code.to_owned())),
        source: Some(DIAGNOSTIC_SOURCE.to_owned()),
        message,
        ..Default::default()
    }
}

#[derive(Clone, Debug)]
struct Token {
    kind: TokenKind,
    span: std::ops::Range<usize>,
}

impl Token {
    fn identifier(&self) -> Option<&str> {
        match &self.kind {
            TokenKind::Identifier(identifier) => Some(identifier),
            _ => None,
        }
    }

    fn string_literal(&self) -> Option<&StringLiteral> {
        match &self.kind {
            TokenKind::StringLiteral(literal) => Some(literal),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
enum TokenKind {
    Identifier(String),
    StringLiteral(StringLiteral),
    Dot,
    LeftParen,
    RightParen,
    Comma,
    Other,
}

#[derive(Clone, Debug)]
struct StringLiteral {
    value: Option<String>,
    quote: char,
    content_start: usize,
    span: std::ops::Range<usize>,
    terminated: bool,
}

#[derive(Default)]
struct Lexed {
    tokens: Vec<Token>,
    ended_in_comment_or_template: bool,
    ended_in_quoted_string: bool,
}

fn lex(source: &str) -> Lexed {
    let mut lexed = Lexed::default();
    let mut offset = 0;
    let mut can_start_regex = true;

    while offset < source.len() {
        let (character, character_len) = next_char(source, offset);
        if character.is_whitespace() {
            offset += character_len;
            continue;
        }

        if source[offset..].starts_with("//") {
            match source[offset + 2..].find('\n') {
                Some(relative_end) => {
                    offset += 2 + relative_end + 1;
                    can_start_regex = true;
                    continue;
                }
                None => {
                    lexed.ended_in_comment_or_template = true;
                    break;
                }
            }
        }
        if source[offset..].starts_with("/*") {
            match source[offset + 2..].find("*/") {
                Some(relative_end) => {
                    offset += 2 + relative_end + 2;
                    continue;
                }
                None => {
                    lexed.ended_in_comment_or_template = true;
                    break;
                }
            }
        }

        if character == '`' {
            let (end, terminated) = skip_template(source, offset);
            lexed.tokens.push(Token {
                kind: TokenKind::Other,
                span: offset..end,
            });
            offset = end;
            if !terminated {
                lexed.ended_in_comment_or_template = true;
                break;
            }
            can_start_regex = false;
            continue;
        }

        if matches!(character, '\'' | '"') {
            let literal = parse_string_literal(source, offset, character);
            let end = literal.span.end;
            let terminated = literal.terminated;
            lexed.tokens.push(Token {
                span: literal.span.clone(),
                kind: TokenKind::StringLiteral(literal),
            });
            offset = end;
            can_start_regex = false;
            if !terminated {
                lexed.ended_in_quoted_string = true;
                break;
            }
            continue;
        }

        if character == '/' && can_start_regex {
            let (end, terminated) = skip_regex(source, offset);
            if end > offset + 1 {
                lexed.tokens.push(Token {
                    kind: TokenKind::Other,
                    span: offset..end,
                });
                offset = end;
                if !terminated {
                    lexed.ended_in_comment_or_template = true;
                    break;
                }
                can_start_regex = false;
                continue;
            }
        }

        if is_identifier_start(character) {
            let start = offset;
            offset += character_len;
            while offset < source.len() {
                let (next, next_len) = next_char(source, offset);
                if !is_identifier_continue(next) {
                    break;
                }
                offset += next_len;
            }
            let identifier = source[start..offset].to_owned();
            can_start_regex = keyword_allows_regex_after(&identifier);
            lexed.tokens.push(Token {
                kind: TokenKind::Identifier(identifier),
                span: start..offset,
            });
            continue;
        }

        let kind = match character {
            '.' => TokenKind::Dot,
            '(' => TokenKind::LeftParen,
            ')' => TokenKind::RightParen,
            ',' => TokenKind::Comma,
            _ => TokenKind::Other,
        };
        can_start_regex = matches!(
            character,
            '(' | '['
                | '{'
                | ','
                | ';'
                | ':'
                | '='
                | '!'
                | '?'
                | '+'
                | '-'
                | '*'
                | '%'
                | '&'
                | '|'
                | '^'
                | '~'
                | '<'
                | '>'
        );
        lexed.tokens.push(Token {
            kind,
            span: offset..offset + character_len,
        });
        offset += character_len;
    }

    lexed
}

fn is_identifier_start(character: char) -> bool {
    character == '_' || character == '$' || character.is_alphabetic()
}

fn is_identifier_continue(character: char) -> bool {
    is_identifier_start(character) || character.is_numeric()
}

fn keyword_allows_regex_after(identifier: &str) -> bool {
    matches!(
        identifier,
        "return"
            | "throw"
            | "case"
            | "delete"
            | "void"
            | "typeof"
            | "instanceof"
            | "in"
            | "of"
            | "new"
            | "yield"
            | "await"
    )
}

fn next_char(source: &str, offset: usize) -> (char, usize) {
    let character = source[offset..]
        .chars()
        .next()
        .expect("offset must be within source");
    (character, character.len_utf8())
}

fn parse_string_literal(source: &str, start: usize, quote: char) -> StringLiteral {
    let content_start = start + quote.len_utf8();
    let mut offset = content_start;
    let mut decoded = String::new();
    let mut valid = true;
    let mut terminated = false;

    while offset < source.len() {
        let (character, character_len) = next_char(source, offset);
        if character == quote {
            offset += character_len;
            terminated = true;
            break;
        }
        if matches!(character, '\n' | '\r') {
            valid = false;
            break;
        }
        if character != '\\' {
            decoded.push(character);
            offset += character_len;
            continue;
        }

        offset += character_len;
        if offset >= source.len() {
            valid = false;
            break;
        }
        let (escape, escape_len) = next_char(source, offset);
        offset += escape_len;
        match escape {
            '\\' | '\'' | '"' => decoded.push(escape),
            'n' => decoded.push('\n'),
            'r' => decoded.push('\r'),
            't' => decoded.push('\t'),
            'b' => decoded.push('\u{0008}'),
            'f' => decoded.push('\u{000c}'),
            'v' => decoded.push('\u{000b}'),
            '0' => decoded.push('\0'),
            '\n' => {}
            '\r' => {
                if source[offset..].starts_with('\n') {
                    offset += 1;
                }
            }
            'x' => {
                let Some((value, end)) = parse_hex_escape(source, offset, 2) else {
                    valid = false;
                    continue;
                };
                offset = end;
                valid &= char::from_u32(value).is_some_and(|value| {
                    decoded.push(value);
                    true
                });
            }
            'u' => {
                let parsed = if source[offset..].starts_with('{') {
                    parse_braced_unicode_escape(source, offset)
                } else {
                    parse_hex_escape(source, offset, 4)
                };
                let Some((value, end)) = parsed else {
                    valid = false;
                    continue;
                };
                offset = end;
                valid &= char::from_u32(value).is_some_and(|value| {
                    decoded.push(value);
                    true
                });
            }
            // JavaScript permits escaped non-special characters. Their value
            // is simply the escaped character.
            other => decoded.push(other),
        }
    }

    StringLiteral {
        value: valid.then_some(decoded),
        quote,
        content_start,
        span: start..offset,
        terminated,
    }
}

fn parse_hex_escape(source: &str, start: usize, digits: usize) -> Option<(u32, usize)> {
    let end = start.checked_add(digits)?;
    let text = source.get(start..end)?;
    if !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(text, 16).ok().map(|value| (value, end))
}

fn parse_braced_unicode_escape(source: &str, start: usize) -> Option<(u32, usize)> {
    let body_start = start + 1;
    let relative_end = source.get(body_start..)?.find('}')?;
    let body_end = body_start + relative_end;
    let text = source.get(body_start..body_end)?;
    if text.is_empty() || text.len() > 6 || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(text, 16)
        .ok()
        .map(|value| (value, body_end + 1))
}

fn skip_template(source: &str, start: usize) -> (usize, bool) {
    let mut offset = start + 1;
    let mut escaped = false;
    while offset < source.len() {
        let (character, character_len) = next_char(source, offset);
        offset += character_len;
        if escaped {
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == '`' {
            return (offset, true);
        }
    }
    (offset, false)
}

fn skip_regex(source: &str, start: usize) -> (usize, bool) {
    let mut offset = start + 1;
    let mut escaped = false;
    let mut in_character_class = false;
    while offset < source.len() {
        let (character, character_len) = next_char(source, offset);
        offset += character_len;
        if matches!(character, '\n' | '\r') {
            return (offset, false);
        }
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' => escaped = true,
            '[' => in_character_class = true,
            ']' => in_character_class = false,
            '/' if !in_character_class => {
                while offset < source.len() {
                    let (flag, flag_len) = next_char(source, offset);
                    if !flag.is_alphabetic() {
                        break;
                    }
                    offset += flag_len;
                }
                return (offset, true);
            }
            _ => {}
        }
    }
    (offset, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{Position, Range};

    fn labels(items: Vec<CompletionItem>) -> Vec<String> {
        items.into_iter().map(|item| item.label).collect()
    }

    fn completion_text_edit(item: &CompletionItem) -> &TextEdit {
        match item.text_edit.as_ref() {
            Some(CompletionTextEdit::Edit(edit)) => edit,
            other => panic!("expected a plain completion text edit, got {other:?}"),
        }
    }

    fn completion_documentation(item: &CompletionItem) -> &str {
        match item.documentation.as_ref() {
            Some(Documentation::String(documentation)) => documentation,
            Some(Documentation::MarkupContent(markup)) => &markup.value,
            None => panic!("expected completion documentation for {:?}", item.label),
        }
    }

    fn hover_markdown(hover: &Hover) -> &str {
        match &hover.contents {
            HoverContents::Markup(markup) => &markup.value,
            other => panic!("expected Markdown hover content, got {other:?}"),
        }
    }

    fn hover_at(provider: &ScriptCompletionProvider, source: &str, needle: &str) -> Hover {
        let offset = source.rfind(needle).expect("hover needle") + 1;
        provider
            .hover_for_source(source, offset)
            .unwrap_or_else(|| panic!("expected hover for {needle:?} in {source:?}"))
    }

    fn catalog() -> ScriptVariableCatalog {
        ScriptVariableCatalog::from_names(
            ["api_token", "base_url", "shared"],
            std::iter::empty::<String>(),
            ["collection_id", "shared"],
        )
    }

    fn provider(
        phase: ScriptEditorPhase,
        catalog: ScriptVariableCatalog,
    ) -> ScriptCompletionProvider {
        ScriptCompletionProvider::new(phase, catalog.shared())
    }

    #[test]
    fn plain_snippets_use_an_isolated_typescript_document() {
        let provider = ScriptCompletionProvider::for_plain_snippet(
            ScriptEditorPhase::PreRequest,
            ScriptVariableCatalog::default().shared(),
        );
        assert_eq!(
            provider.typescript_document,
            TypeScriptDocumentKind::PlainSnippet(TypeScriptScriptPhase::PreRequest)
        );
    }

    #[test]
    fn interactive_console_has_an_isolated_post_response_document() {
        let provider = ScriptCompletionProvider::for_interactive_console(
            ScriptVariableCatalog::default().shared(),
        );
        assert_eq!(
            provider.typescript_document,
            TypeScriptDocumentKind::InteractiveConsole
        );
        let items = provider.completion_items_for_source("api.", 4);
        assert!(items.iter().any(|item| item.label == "response"));
        assert!(items.iter().any(|item| item.label == "test"));
    }

    #[test]
    fn catalog_orders_names_and_redacts_values_from_debug_output() {
        let catalog = ScriptVariableCatalog::from_environment_values(
            [
                ("zeta", "super-secret-value"),
                ("", "ignored"),
                ("alpha", "visible-in-editor"),
            ],
            std::iter::empty::<String>(),
            ["shared", "shared"],
        );

        assert_eq!(
            catalog.environment_names().collect::<Vec<_>>(),
            ["alpha", "zeta"]
        );
        assert_eq!(
            catalog
                .collection_names
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["shared"]
        );
        assert_eq!(
            catalog.variable_names().collect::<Vec<_>>(),
            ["alpha", "shared", "zeta"]
        );
        assert_eq!(
            catalog.environment_values.get("zeta").map(String::as_str),
            Some("super-secret-value")
        );

        let debug = format!("{catalog:?}");
        assert!(debug.contains("environment_value_count: 2"));
        for value in ["super-secret-value", "visible-in-editor"] {
            assert!(!debug.contains(value));
        }
    }

    #[test]
    fn completion_members_follow_the_runtime_and_phase() {
        let pre = provider(ScriptEditorPhase::PreRequest, catalog());
        let post = provider(ScriptEditorPhase::PostResponse, catalog());

        let pre_api = labels(pre.completion_items_for_source("api.", 4));
        assert!(pre_api.contains(&"request".to_owned()));
        assert!(!pre_api.contains(&"response".to_owned()));
        assert!(!pre_api.contains(&"test".to_owned()));

        let post_api = labels(post.completion_items_for_source("api.", 4));
        assert!(post_api.contains(&"response".to_owned()));
        assert!(post_api.contains(&"test".to_owned()));
        assert!(post_api.contains(&"assert".to_owned()));

        let mutable_headers = labels(pre.completion_items_for_source("api.request.headers.", 20));
        assert!(mutable_headers.contains(&"set".to_owned()));
        assert!(mutable_headers.contains(&"append".to_owned()));

        let readonly_headers = labels(post.completion_items_for_source("api.request.headers.", 20));
        assert!(!readonly_headers.contains(&"set".to_owned()));
        assert!(readonly_headers.contains(&"getAll".to_owned()));

        let later_global = labels(pre.completion_items_for_source("const value = 1;\nap", 19));
        assert_eq!(later_global, ["api"]);
    }

    #[test]
    fn completion_trigger_uses_a_bounded_relevant_context() {
        let variables = catalog();

        let ordinary = Rope::from("const ordinaryName = 1");
        assert!(!script_completion_is_active(
            &ordinary,
            ordinary.len(),
            ScriptEditorPhase::PreRequest,
            &variables,
            None,
        ));

        let member = Rope::from("api.request.");
        assert!(script_completion_is_active(
            &member,
            member.len(),
            ScriptEditorPhase::PreRequest,
            &variables,
            None,
        ));

        let pre_response = Rope::from("api.response.");
        assert!(!script_completion_is_active(
            &pre_response,
            pre_response.len(),
            ScriptEditorPhase::PreRequest,
            &variables,
            None,
        ));
        assert!(script_completion_is_active(
            &pre_response,
            pre_response.len(),
            ScriptEditorPhase::PostResponse,
            &variables,
            None,
        ));

        let variable = Rope::from(r#"api.environment.get("ba"#);
        assert!(script_completion_is_active(
            &variable,
            variable.len(),
            ScriptEditorPhase::PreRequest,
            &variables,
            None,
        ));

        let comment = Rope::from("// api.request.");
        assert!(!script_completion_is_active(
            &comment,
            comment.len(),
            ScriptEditorPhase::PreRequest,
            &variables,
            None,
        ));
    }

    #[test]
    fn completion_offers_available_variable_names() {
        let provider = provider(ScriptEditorPhase::PreRequest, catalog());

        let environment = provider.completion_items_for_source(r#"api.environment.get("ba"#, 23);
        assert_eq!(labels(environment.clone()), ["base_url"]);
        assert!(format!("{environment:?}").contains("base_url"));

        let variables = labels(provider.completion_items_for_source(r#"api.variables.get("c"#, 20));
        assert_eq!(variables, ["collection_id"]);

        let environment_only =
            labels(provider.completion_items_for_source(r#"api.environment.get("c"#, 22));
        assert!(environment_only.is_empty());
    }

    #[test]
    fn request_script_editors_show_values_but_plain_snippets_do_not() {
        let rendered_value = "line `tick`\n**still a value**";
        let variables = ScriptVariableCatalog::from_environment_values(
            [
                ("api_token", rendered_value),
                ("base_url", "https://example.test"),
            ],
            std::iter::empty::<String>(),
            ["collection_id"],
        )
        .shared();
        let source = r#"api.environment.get("api"#;
        let hover_source = r#"api.environment.get("api_token")"#;

        for phase in [
            ScriptEditorPhase::PreRequest,
            ScriptEditorPhase::PostResponse,
        ] {
            let provider = ScriptCompletionProvider::new(phase, variables.clone());
            let items = provider.completion_items_for_source(source, source.len());
            assert_eq!(labels(items.clone()), ["api_token"]);
            let documentation = completion_documentation(&items[0]);
            assert!(documentation.contains("Selected environment value"));
            assert!(documentation.contains(r#"line `tick`\n**still a value**"#));
            assert!(!documentation.contains("line `tick`\n**still a value**"));

            let hover = hover_at(&provider, hover_source, "api_token");
            let markdown = hover_markdown(&hover);
            assert!(markdown.contains("Selected environment value"));
            assert!(markdown.contains(r#"line `tick`\n**still a value**"#));
            assert!(!markdown.contains("line `tick`\n**still a value**"));
        }

        let plain =
            ScriptCompletionProvider::for_plain_snippet(ScriptEditorPhase::PreRequest, variables);
        let items = plain.completion_items_for_source(source, source.len());
        assert_eq!(labels(items.clone()), ["api_token"]);
        assert!(!completion_documentation(&items[0]).contains("Selected environment value"));
        assert!(!completion_documentation(&items[0]).contains(rendered_value));
        let hover = hover_at(&plain, hover_source, "api_token");
        assert!(!hover_markdown(&hover).contains("Selected environment value"));
        assert!(!hover_markdown(&hover).contains(rendered_value));

        let repeated_set = concat!(
            "api.environment.set(\"api_token\", firstValue);\n",
            "api.environment.set(\"api_token\", secondValue);",
        );
        let provider = ScriptCompletionProvider::new(
            ScriptEditorPhase::PostResponse,
            ScriptVariableCatalog::from_environment_values(
                [("api_token", "configured-token")],
                std::iter::empty::<String>(),
                std::iter::empty::<String>(),
            )
            .shared(),
        );
        let hover = hover_at(&provider, repeated_set, "api_token");
        let markdown = hover_markdown(&hover);
        assert!(markdown.contains("Selected environment value"));
        assert!(markdown.contains("configured-token"));

        let unset_then_read = concat!(
            "api.environment.unset(\"api_token\");\n",
            "api.environment.get(\"api_token\");",
        );
        let hover = hover_at(&provider, unset_then_read, "api_token");
        let markdown = hover_markdown(&hover);
        assert!(markdown.contains("Selected environment value"));
        assert!(markdown.contains("configured-token"));
    }

    #[test]
    fn variable_completion_applies_mutated_names_but_keeps_configured_values() {
        let variables = ScriptVariableCatalog::from_environment_values(
            [
                ("api_token", "stored-secret"),
                ("shared", "environment-value"),
            ],
            std::iter::empty::<String>(),
            ["shared"],
        )
        .shared();
        let provider = ScriptCompletionProvider::new(ScriptEditorPhase::PreRequest, variables);

        let unset_environment = concat!(
            "api.environment.unset(\"api_token\");\n",
            "api.environment.get(\"api",
        );
        assert!(
            provider
                .completion_items_for_source(unset_environment, unset_environment.len())
                .is_empty(),
            "an unset environment key must no longer be offered",
        );

        let collection_fallback = concat!(
            "api.environment.unset(\"shared\");\n",
            "api.variables.get(\"sha",
        );
        let items =
            provider.completion_items_for_source(collection_fallback, collection_fallback.len());
        assert_eq!(labels(items.clone()), ["shared"]);
        assert_eq!(items[0].detail.as_deref(), Some("collection variable"));
        assert!(!completion_documentation(&items[0]).contains("environment-value"));

        let created = concat!(
            "api.environment.set(\"later\", \"created-in-script\");\n",
            "api.environment.get(\"lat",
        );
        let items = provider.completion_items_for_source(created, created.len());
        assert_eq!(labels(items.clone()), ["later"]);
        assert_eq!(
            items[0].detail.as_deref(),
            Some("selected-environment variable")
        );
        assert!(!completion_documentation(&items[0]).contains("Selected environment value"));
        assert!(!completion_documentation(&items[0]).contains("created-in-script"));

        let overwritten = concat!(
            "api.environment.set(\"api_token\", computeToken());\n",
            "api.environment.get(\"api",
        );
        let items = provider.completion_items_for_source(overwritten, overwritten.len());
        assert_eq!(labels(items.clone()), ["api_token"]);
        let documentation = completion_documentation(&items[0]);
        assert!(documentation.contains("Selected environment value"));
        assert!(documentation.contains("stored-secret"));
    }

    #[test]
    fn request_literal_completions_cover_runtime_enum_values() {
        let provider = provider(ScriptEditorPhase::PreRequest, catalog());

        let methods = r#"api.request.method = ""#;
        assert_eq!(
            labels(provider.completion_items_for_source(methods, methods.len())),
            STANDARD_HTTP_METHODS
        );

        let body_mode = r#"api.request.bodyMode = "m"#;
        assert_eq!(
            labels(provider.completion_items_for_source(body_mode, body_mode.len())),
            ["multipart_form_data"]
        );

        let language = r#"api.request.rawBodyLanguage = 'type"#;
        assert_eq!(
            labels(provider.completion_items_for_source(language, language.len())),
            ["typescript"]
        );

        for source in [
            r#"api.request.bodyFields[0].kind = "f"#,
            r#"api.request.bodyFields[ fieldIndex ].kind = "f"#,
            r#"api.request.bodyFields.push({ enabled: true, kind: "f"#,
        ] {
            assert_eq!(
                labels(provider.completion_items_for_source(source, source.len())),
                ["file"],
                "unexpected body-field completion for {source:?}"
            );
        }
    }

    #[test]
    fn request_literal_completion_replaces_only_the_quoted_prefix() {
        let provider = provider(ScriptEditorPhase::PreRequest, catalog());
        let source = "const şehir = 1;\napi.request.method = 'po';";
        let cursor = source.find("'po").expect("quoted method") + "'po".len();
        let items = provider.completion_items_for_source(source, cursor);

        assert_eq!(labels(items.clone()), ["POST"]);
        let edit = completion_text_edit(&items[0]);
        assert_eq!(edit.range.start, Position::new(1, 22));
        assert_eq!(edit.range.end, Position::new(1, 24));
        assert_eq!(edit.new_text, "POST");
        assert_eq!(&source[cursor..], "';");
    }

    #[test]
    fn request_literal_completion_replaces_an_existing_value_suffix() {
        let provider = provider(ScriptEditorPhase::PreRequest, catalog());
        let source = r#"api.request.method = "PST";"#;
        let content_start = source.find("PST").expect("method value");
        let cursor = content_start + 1;
        let items = provider.completion_items_for_source(source, cursor);

        assert_eq!(labels(items.clone()), ["POST", "PUT", "PATCH"]);
        let edit = completion_text_edit(&items[0]);
        assert_eq!(edit.range.start, Position::new(0, 22));
        assert_eq!(edit.range.end, Position::new(0, 25));
        assert_eq!(edit.new_text, "POST");
        assert_eq!(
            format!(
                "{}{}{}",
                &source[..content_start],
                edit.new_text,
                &source[content_start + 3..]
            ),
            r#"api.request.method = "POST";"#
        );
    }

    #[test]
    fn request_literal_completions_reject_readonly_and_unrelated_strings() {
        let pre = provider(ScriptEditorPhase::PreRequest, catalog());
        let post = provider(ScriptEditorPhase::PostResponse, catalog());

        for source in [
            r#"api.request.method == "G"#,
            r#"api.request.method += "G"#,
            r#"foo.api.request.method = "G"#,
            r#"api.request.url = "h"#,
            r#"api.request.bodyFields[i + 1].kind = "f"#,
            r#"rows.push({ kind: "f"#,
            r#"// api.request.method = "G"#,
            r#"const example = `api.request.method = "G`"#,
            r#"api.request.method = "PURG"#,
        ] {
            assert!(
                pre.completion_items_for_source(source, source.len())
                    .is_empty(),
                "unexpected request literal completion for {source:?}"
            );
        }

        let readonly = r#"api.request.bodyMode = "r"#;
        assert!(
            post.completion_items_for_source(readonly, readonly.len())
                .is_empty()
        );
    }

    #[test]
    fn request_literal_completion_trigger_is_context_aware() {
        let variables = catalog();
        for source in [
            r#"api.request.method = ""#,
            r#"api.request.bodyMode = "form"#,
            r#"api.request.rawBodyLanguage = "ja"#,
            r#"api.request.bodyFields[0].kind = "t"#,
        ] {
            let rope = Rope::from(source);
            assert!(
                script_completion_is_active(
                    &rope,
                    rope.len(),
                    ScriptEditorPhase::PreRequest,
                    &variables,
                    None,
                ),
                "completion should be active for {source:?}"
            );
        }

        for source in [
            r#"api.request.method = "PURG"#,
            r#"api.request.url = "https"#,
        ] {
            let rope = Rope::from(source);
            assert!(
                !script_completion_is_active(
                    &rope,
                    rope.len(),
                    ScriptEditorPhase::PreRequest,
                    &variables,
                    None,
                ),
                "completion should be inactive for {source:?}"
            );
        }

        let readonly = Rope::from(r#"api.request.method = "G"#);
        assert!(!script_completion_is_active(
            &readonly,
            readonly.len(),
            ScriptEditorPhase::PostResponse,
            &variables,
            None,
        ));
    }

    #[test]
    fn diagnostics_warn_only_for_missing_literal_reads() {
        let source = r#"
api.environment.get("base_url");
api.environment.get("missing_env");
api.variables.get('collection_id');
api.variables.get('missing_anywhere');
"#;
        let diagnostics = diagnostics_for_source(source, &catalog(), None);

        assert_eq!(diagnostics.len(), 2);
        assert!(diagnostics[0].message.contains("missing_env"));
        assert!(diagnostics[1].message.contains("missing_anywhere"));
        assert!(
            diagnostics
                .iter()
                .all(|diagnostic| diagnostic.severity == Some(DiagnosticSeverity::WARNING))
        );
    }

    #[test]
    fn diagnostics_skip_comments_strings_dynamic_has_and_writes() {
        let source = r#"
// api.environment.get("commented");
/* api.variables.get("also_commented"); */
const example = 'api.environment.get("inside_string")';
const matcher = /api\.environment\.get\("inside_regex"\)/;
api.environment.get(variableName);
api.environment.has("missing_has");
api.environment.set("created", "secret-value");
api.environment.unset("missing_unset");
"#;

        assert!(diagnostics_for_source(source, &catalog(), None).is_empty());
    }

    #[test]
    fn preceding_literal_set_satisfies_later_reads() {
        let source = r#"
api.environment.get("before");
api.environment.set("after", computeValue());
api.environment.get("after");
api.variables.get("after");
"#;
        let diagnostics = diagnostics_for_source(source, &catalog(), None);

        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].message.contains("before"));
    }

    #[test]
    fn disabled_names_are_not_completed_and_have_a_distinct_warning() {
        let catalog = ScriptVariableCatalog::from_names(["enabled"], ["disabled"], ["collection"]);
        let provider = provider(ScriptEditorPhase::PreRequest, catalog.clone());

        let source = r#"api.environment.get("d"#;
        assert!(
            provider
                .completion_items_for_source(source, source.len())
                .is_empty()
        );

        let diagnostics =
            diagnostics_for_source(r#"api.environment.get("disabled")"#, &catalog, None);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].code,
            Some(NumberOrString::String(DISABLED_VARIABLE_CODE.to_owned()))
        );
        assert!(diagnostics[0].message.contains("is disabled"));
    }

    #[test]
    fn shared_catalog_updates_are_visible_without_replacing_the_provider() {
        let catalog = ScriptVariableCatalog::from_names(
            ["old_name"],
            std::iter::empty::<String>(),
            std::iter::empty::<String>(),
        )
        .shared();
        let provider =
            ScriptCompletionProvider::new(ScriptEditorPhase::PreRequest, catalog.clone());

        let source = r#"api.environment.get("n"#;
        assert!(
            provider
                .completion_items_for_source(source, source.len())
                .is_empty()
        );

        catalog.borrow_mut().replace(
            ["new_name"],
            std::iter::empty::<String>(),
            std::iter::empty::<String>(),
        );

        assert_eq!(
            labels(provider.completion_items_for_source(source, source.len())),
            ["new_name"]
        );
    }

    #[test]
    fn same_script_set_must_precede_the_read_and_be_literal() {
        let source = r#"
api.environment.set(dynamicName, "value");
api.environment.get("dynamic");
api.environment.get("later");
api.environment.set("later", "value");
"#;
        let diagnostics = diagnostics_for_source(source, &catalog(), None);

        assert_eq!(diagnostics.len(), 2);
        assert!(diagnostics[0].message.contains("dynamic"));
        assert!(diagnostics[1].message.contains("later"));
    }

    #[test]
    fn completion_and_diagnostics_handle_unicode_names() {
        let catalog = ScriptVariableCatalog::from_names(
            ["şehir"],
            std::iter::empty::<String>(),
            std::iter::empty::<String>(),
        );
        let provider = provider(ScriptEditorPhase::PreRequest, catalog.clone());
        let source = r#"api.environment.get("şe"#;
        let items = provider.completion_items_for_source(source, source.len());

        assert_eq!(labels(items), ["şehir"]);
        assert!(
            diagnostics_for_source(r#"api.environment.get("şehir")"#, &catalog, None).is_empty()
        );
    }

    #[test]
    fn hover_documents_runtime_function_parameters_and_fields() {
        let pre = provider(ScriptEditorPhase::PreRequest, catalog());
        let post = provider(ScriptEditorPhase::PostResponse, catalog());

        let header_source = r#"api.request.headers.set("X-Token", "value");"#;
        let header_hover = hover_at(&pre, header_source, "set");
        let header_markdown = hover_markdown(&header_hover);
        assert!(
            header_markdown
                .contains("api.request.headers.set(name: unknown, value: unknown): void")
        );
        assert!(header_markdown.contains("removing later enabled duplicates"));
        assert_eq!(
            header_hover.range,
            Some(Range::new(Position::new(0, 20), Position::new(0, 23)))
        );

        let status_hover = hover_at(&post, "api.response.status;", "status");
        let status_markdown = hover_markdown(&status_hover);
        assert!(status_markdown.contains("api.response.status: number"));
        assert!(status_markdown.contains("HTTP response status code"));

        let test_hover = hover_at(
            &post,
            r#"api.test("status", () => api.assert(true));"#,
            "test",
        );
        assert!(
            hover_markdown(&test_hover)
                .contains("api.test(name: unknown, callback: () => unknown): void")
        );
        assert!(hover_markdown(&test_hover).contains("async callbacks are unsupported"));

        let console_hover = hover_at(&pre, "console.log(api.request.url);", "log");
        assert!(hover_markdown(&console_hover).contains("console.log(...values: unknown[]): void"));
    }

    #[test]
    fn hover_respects_script_phase_and_request_mutability() {
        let pre = provider(ScriptEditorPhase::PreRequest, catalog());
        let post = provider(ScriptEditorPhase::PostResponse, catalog());

        for source in ["api.response", "api.test", "api.assert"] {
            let offset = source.rfind('.').expect("member separator") + 2;
            assert!(
                pre.hover_for_source(source, offset).is_none(),
                "pre-request hover should hide {source}"
            );
        }
        assert!(
            post.hover_for_source("api.request.headers.set", 22)
                .is_none()
        );

        let pre_headers = hover_at(&pre, "api.request.headers", "headers");
        assert!(hover_markdown(&pre_headers).contains("api.request.headers: MutableHeaders"));
        let post_headers = hover_at(&post, "api.request.headers", "headers");
        assert!(hover_markdown(&post_headers).contains("api.request.headers: ReadonlyHeaders"));

        let response = hover_at(&post, "api.response.json()", "json");
        assert!(hover_markdown(&response).contains("api.response.json(): any"));
    }

    #[test]
    fn hover_rejects_non_runtime_text_and_expression_suffixes() {
        let post = provider(ScriptEditorPhase::PostResponse, catalog());
        for (source, needle) in [
            ("// api.response.json()", "json"),
            ("/* api.response.json() */", "json"),
            (r#""api.response.json()""#, "json"),
            ("`api.response.json()`", "json"),
            ("const pattern = /api.response.json/;", "json"),
            ("client.api.response.json()", "json"),
            ("getApi().response", "response"),
            (r#"api["response"].json()"#, "json"),
            ("api`tag`.request", "request"),
        ] {
            let offset = source.rfind(needle).expect("hover needle") + 1;
            assert!(
                post.hover_for_source(source, offset).is_none(),
                "unexpected hover for {source:?}"
            );
        }

        let source = "api /* runtime object */ . response";
        let hover = hover_at(&post, source, "response");
        assert!(hover_markdown(&hover).contains("api.response: Response"));
    }

    #[test]
    fn variable_name_hover_reports_scope_without_values() {
        let catalog = ScriptVariableCatalog::from_names(
            ["environment_only", "shared"],
            ["disabled_key"],
            ["collection_only", "shared"],
        );
        let pre = provider(ScriptEditorPhase::PreRequest, catalog);

        let environment = hover_at(
            &pre,
            r#"api.environment.get("environment_only")"#,
            "environment_only",
        );
        assert!(hover_markdown(&environment).contains("Available in the selected environment"));

        let collection = hover_at(
            &pre,
            r#"api.variables.get("collection_only")"#,
            "collection_only",
        );
        assert!(hover_markdown(&collection).contains("Available as a collection variable"));

        let shared = hover_at(&pre, r#"api.variables.get("shared")"#, "shared");
        assert!(hover_markdown(&shared).contains("Available in the selected environment"));

        let disabled = hover_at(
            &pre,
            r#"api.environment.get("disabled_key")"#,
            "disabled_key",
        );
        assert!(
            hover_markdown(&disabled)
                .contains("exists in the selected environment but is disabled")
        );

        let missing = hover_at(&pre, r#"api.variables.get("missing_key")"#, "missing_key");
        assert!(
            hover_markdown(&missing)
                .contains("does not currently exist in the selected environment or collection")
        );
        assert!(!hover_markdown(&missing).contains("super-secret-value"));

        let second_argument = r#"api.environment.set("new_key", "value")"#;
        let value_offset = second_argument.rfind("value").expect("second argument") + 1;
        assert!(
            pre.hover_for_source(second_argument, value_offset)
                .is_none()
        );

        let opening_quote = r#"api.environment.get("environment_only")"#;
        let quote_offset = opening_quote.find('"').expect("opening quote");
        assert!(pre.hover_for_source(opening_quote, quote_offset).is_none());
    }

    #[test]
    fn hover_ranges_use_gpui_unicode_scalar_columns() {
        let post = provider(ScriptEditorPhase::PostResponse, catalog());
        let source = "const şehir = 1; api.response.json();";
        let hover = hover_at(&post, source, "json");

        assert_eq!(
            hover.range,
            Some(Range::new(Position::new(0, 30), Position::new(0, 34)))
        );
        let inside_multibyte = source.find('ş').expect("unicode identifier") + 1;
        assert!(
            post.hover_for_source(source, inside_multibyte).is_none(),
            "clipping an offset inside a multibyte scalar must not panic or resolve a runtime symbol"
        );
    }

    #[test]
    fn hover_accepts_gpui_caret_offsets_at_symbol_boundaries() {
        let pre = provider(ScriptEditorPhase::PreRequest, catalog());
        let source = r#"api.request.headers.set("X-Token", "value");"#;
        let set_start = source.find(".set").expect("set member") + 1;
        let set_end = set_start + "set".len();
        let hover = pre
            .hover_for_source(source, set_end)
            .expect("right half of the final glyph resolves to the identifier on its left");

        assert!(
            hover_markdown(&hover)
                .contains("api.request.headers.set(name: unknown, value: unknown): void")
        );
        assert_eq!(
            hover.range,
            Some(Range::new(Position::new(0, 20), Position::new(0, 23)))
        );

        let api_boundary = pre
            .hover_for_source("api.request", 3)
            .expect("identifier remains hoverable at the following dot boundary");
        assert!(hover_markdown(&api_boundary).contains("api: PreRequestApi"));
        assert_eq!(
            api_boundary.range,
            Some(Range::new(Position::new(0, 0), Position::new(0, 3)))
        );

        let variable = r#"api.environment.get("base_url")"#;
        let closing_quote = variable.rfind('"').expect("closing quote");
        let variable_hover = pre
            .hover_for_source(variable, closing_quote)
            .expect("last variable-name glyph resolves at the closing-quote caret");
        assert!(hover_markdown(&variable_hover).contains("Available in the selected environment"));
        assert_eq!(
            variable_hover.range,
            Some(Range::new(Position::new(0, 21), Position::new(0, 29)))
        );
    }

    #[test]
    fn variable_hover_models_earlier_environment_mutations() {
        let catalog = ScriptVariableCatalog::from_names(
            ["environment_only", "shared"],
            ["disabled_key"],
            ["shared"],
        );
        let pre = provider(ScriptEditorPhase::PreRequest, catalog);

        let created = r#"
api.environment.set("later", "super-secret-value");
api.environment.get("later");
"#;
        let created_hover = hover_at(&pre, created, "later");
        assert!(hover_markdown(&created_hover).contains("Available in the selected environment"));
        assert!(!hover_markdown(&created_hover).contains("super-secret-value"));

        let removed = r#"
api.environment.unset("environment_only");
api.environment.get("environment_only");
"#;
        let removed_hover = hover_at(&pre, removed, "environment_only");
        assert!(
            hover_markdown(&removed_hover)
                .contains("does not currently exist in the selected environment")
        );

        let fallback = r#"
api.environment.unset("shared");
api.variables.get("shared");
"#;
        let fallback_hover = hover_at(&pre, fallback, "shared");
        assert!(hover_markdown(&fallback_hover).contains("Available as a collection variable"));

        let enabled = r#"
api.environment.set("disabled_key", "new-value");
api.environment.get("disabled_key");
"#;
        let enabled_hover = hover_at(&pre, enabled, "disabled_key");
        assert!(hover_markdown(&enabled_hover).contains("Available in the selected environment"));
    }

    // ---- saved-request namespace intelligence ----

    fn request_workspace() -> crate::core::Workspace {
        use crate::core::{RequestDraft, RequestScripts, RequestTemplate};
        let mut workspace = crate::core::Workspace::default();
        let chat = workspace.create_collection("ChatAdmin").unwrap();
        let login = RequestTemplate {
            request: RequestDraft::new("POST", "https://a.test/login"),
            scripts: RequestScripts::default(),
            websocket: None,
        };
        workspace
            .create_saved_request(&chat, "Login", login)
            .unwrap();
        workspace
            .create_saved_request(
                &chat,
                "Logout",
                RequestTemplate {
                    request: RequestDraft::new("POST", "https://a.test/logout"),
                    scripts: RequestScripts::default(),
                    websocket: None,
                },
            )
            .unwrap();
        let users = workspace
            .create_collection_folder(&chat, None, "Users")
            .unwrap();
        workspace
            .create_saved_request_in_folder(
                &chat,
                Some(&users),
                "Create",
                RequestTemplate {
                    request: RequestDraft::new("POST", "https://a.test/users"),
                    scripts: RequestScripts::default(),
                    websocket: None,
                },
            )
            .unwrap();
        let payments = workspace.create_collection("Payments").unwrap();
        workspace
            .create_saved_request(
                &payments,
                "Login",
                RequestTemplate {
                    request: RequestDraft::new("POST", "https://p.test/login"),
                    scripts: RequestScripts::default(),
                    websocket: None,
                },
            )
            .unwrap();
        workspace
    }

    fn namespace_provider(phase: ScriptEditorPhase) -> ScriptCompletionProvider {
        let workspace = request_workspace();
        let catalog = crate::core::RequestNamespaceCatalog::from_workspace(&workspace);
        ScriptCompletionProvider::new(phase, ScriptVariableCatalog::default().shared())
            .with_request_namespace(Rc::new(RefCell::new(catalog)))
    }

    #[test]
    fn request_namespace_completes_root_collections() {
        let provider = namespace_provider(ScriptEditorPhase::PreRequest);
        let labels = |items: Vec<CompletionItem>| {
            items.into_iter().map(|item| item.label).collect::<Vec<_>>()
        };
        // Root-level collection namespaces participate in completion.
        let root_items = labels(provider.completion_items_for_source("Chat", 5));
        assert!(root_items.contains(&"ChatAdmin".to_owned()));
    }

    #[test]
    fn request_namespace_completes_after_collection_dot() {
        let provider = namespace_provider(ScriptEditorPhase::PreRequest);
        let items = provider.completion_items_for_source(
            "api.requests.execute(ChatAdmin.",
            "api.requests.execute(ChatAdmin.".len(),
        );
        let labels = items
            .iter()
            .map(|item| item.label.clone())
            .collect::<Vec<_>>();
        assert!(labels.contains(&"Login".to_owned()), "got {labels:?}");
        assert!(labels.contains(&"Logout".to_owned()), "got {labels:?}");
        assert!(labels.contains(&"Users".to_owned()), "got {labels:?}");
        // Request completions carry useful non-secret detail (method + template URL).
        let login = items.iter().find(|item| item.label == "Login").unwrap();
        let detail = login.detail.as_deref().unwrap_or_default().to_owned();
        assert!(detail.contains("POST"), "{detail}");
        assert!(detail.contains("https://a.test/login"), "{detail}");
        assert!(detail.contains("Saved request"), "{detail}");
    }

    #[test]
    fn request_namespace_completes_nested_folders() {
        let provider = namespace_provider(ScriptEditorPhase::PostResponse);
        let items = provider.completion_items_for_source(
            "api.requests.execute(ChatAdmin.Users.",
            "api.requests.execute(ChatAdmin.Users.".len(),
        );
        let labels = items
            .iter()
            .map(|item| item.label.clone())
            .collect::<Vec<_>>();
        assert!(labels.contains(&"Create".to_owned()), "got {labels:?}");
    }

    #[test]
    fn api_requests_exposes_execute() {
        let provider = namespace_provider(ScriptEditorPhase::PreRequest);
        let items = provider.completion_items_for_source("api.requests.", "api.requests.".len());
        assert!(
            items.iter().any(|item| item.label == "execute"),
            "api.requests. should offer execute: {:?}",
            items
                .iter()
                .map(|item| item.label.as_str())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn request_reference_hover_shows_metadata_without_secrets() {
        let provider = namespace_provider(ScriptEditorPhase::PreRequest);
        let source = "api.requests.execute(ChatAdmin.Login);";
        let offset = source.rfind("Login").unwrap() + 1;
        let hover = provider
            .hover_for_source(source, offset)
            .expect("hover for ChatAdmin.Login");
        let markdown = hover_markdown(&hover);
        assert!(markdown.contains("Saved request reference"), "{markdown}");
        assert!(markdown.contains("POST"), "{markdown}");
        assert!(markdown.contains("https://a.test/login"), "{markdown}");
        assert!(markdown.contains("api.requests.execute"), "{markdown}");
        // No resolved environment/secret value is ever exposed.
        assert!(!markdown.contains("Bearer"), "{markdown}");
    }

    #[test]
    fn stale_request_reference_is_diagnosed() {
        let workspace = request_workspace();
        let catalog = crate::core::RequestNamespaceCatalog::from_workspace(&workspace);
        let variables = ScriptVariableCatalog::default();
        // A path that no longer exists in the active workspace is flagged.
        let diagnostics = diagnostics_for_source(
            r#"api.requests.execute(ChatAdmin.Gone);"#,
            &variables,
            Some(&catalog),
        );
        assert_eq!(diagnostics.len(), 1);
        assert!(
            diagnostics[0].message.contains("no longer available"),
            "{}",
            diagnostics[0].message
        );
        // A valid reference stays clean.
        assert!(
            diagnostics_for_source(
                r#"api.requests.execute(ChatAdmin.Login);"#,
                &variables,
                Some(&catalog),
            )
            .is_empty()
        );
    }

    #[test]
    fn request_namespace_completion_never_exposes_resolved_values() {
        let provider = namespace_provider(ScriptEditorPhase::PreRequest);
        let items = provider.completion_items_for_source(
            "api.requests.execute(ChatAdmin.",
            "api.requests.execute(ChatAdmin.".len(),
        );
        for item in items {
            let detail = item.detail.as_deref().unwrap_or_default().to_owned();
            let doc = completion_documentation(&item);
            // The template URL may contain placeholder names but never resolved
            // secret text.
            assert!(!detail.contains("secret"), "{detail}");
            assert!(!doc.contains("secret"), "{doc}");
        }
    }
}

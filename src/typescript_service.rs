//! Embedded JavaScript intelligence backed by Microsoft's TypeScript service.
//!
//! This module is deliberately a thin, in-memory host around the official
//! TypeScript LanguageService. It does not implement JavaScript parsing, type
//! inference, or the Language Server Protocol. The TypeScript engine runs in a
//! dedicated QuickJS runtime on its own thread and never shares state with the
//! sandboxes used to execute request scripts or snippet generators.

use std::{
    borrow::Cow,
    ops::Range as ByteRange,
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, Sender},
    },
    thread::{self, JoinHandle},
};

use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionTextEdit, Diagnostic, DiagnosticSeverity, Hover,
    HoverContents, MarkupContent, MarkupKind, NumberOrString, Position, Range, TextEdit,
};
use rquickjs::{CatchResultExt as _, Context, Function, Object, Runtime};
use serde::Deserialize;
use thiserror::Error;
use tokio::sync::oneshot;

use crate::core::{GENERATOR_WRAPPER_PREFIX, GENERATOR_WRAPPER_SUFFIX};

pub const EMBEDDED_TYPESCRIPT_VERSION: &str = "6.0.2";

const TYPESCRIPT_MEMORY_LIMIT_BYTES: usize = 256 * 1024 * 1024;
const TYPESCRIPT_STACK_LIMIT_BYTES: usize = 4 * 1024 * 1024;
const MAX_DOCUMENT_BYTES: usize = 256 * 1024;
const SERVICE_GLOBAL: &str = "__resolvedTypeScriptService";

const TYPESCRIPT_SOURCE: &str =
    include_str!("../vendor/typescript-service-6.0.2/lib/typescript.js");
const BRIDGE_SOURCE: &str = include_str!("typescript_service/bridge.js");
const RUNTIME_DECLARATIONS: &str = include_str!("typescript_service/resolved-runtime.d.ts");
const PRE_REQUEST_DECLARATIONS: &str = include_str!("typescript_service/pre-request.d.ts");
const POST_RESPONSE_DECLARATIONS: &str = include_str!("typescript_service/post-response.d.ts");
const SNIPPET_GENERATOR_DECLARATIONS: &str =
    include_str!("typescript_service/snippet-generator.d.ts");
const EXECUTABLE_SNIPPET_PRE_DECLARATIONS: &str =
    include_str!("typescript_service/executable-snippet-pre-request.d.ts");
const EXECUTABLE_SNIPPET_POST_DECLARATIONS: &str =
    include_str!("typescript_service/executable-snippet-post-response.d.ts");

macro_rules! typescript_library {
    ($file:literal) => {
        (
            concat!("/", $file),
            include_str!(concat!("../vendor/typescript-service-6.0.2/lib/", $file)),
        )
    };
}

// ECMAScript declarations through ES2022 are visible to the service, plus the
// typed-array base64/hex methods implemented by the bundled QuickJS runtime.
// DOM and Node declarations are intentionally absent because those hosts are
// not part of Resolved's script runtime.
const TYPESCRIPT_LIBRARIES: &[(&str, &str)] = &[
    typescript_library!("lib.decorators.d.ts"),
    typescript_library!("lib.decorators.legacy.d.ts"),
    typescript_library!("lib.es5.d.ts"),
    typescript_library!("lib.es2015.d.ts"),
    typescript_library!("lib.es2015.collection.d.ts"),
    typescript_library!("lib.es2015.core.d.ts"),
    typescript_library!("lib.es2015.generator.d.ts"),
    typescript_library!("lib.es2015.iterable.d.ts"),
    typescript_library!("lib.es2015.promise.d.ts"),
    typescript_library!("lib.es2015.proxy.d.ts"),
    typescript_library!("lib.es2015.reflect.d.ts"),
    typescript_library!("lib.es2015.symbol.d.ts"),
    typescript_library!("lib.es2015.symbol.wellknown.d.ts"),
    typescript_library!("lib.es2016.d.ts"),
    typescript_library!("lib.es2016.array.include.d.ts"),
    typescript_library!("lib.es2017.d.ts"),
    typescript_library!("lib.es2017.arraybuffer.d.ts"),
    typescript_library!("lib.es2017.date.d.ts"),
    typescript_library!("lib.es2017.object.d.ts"),
    typescript_library!("lib.es2017.sharedmemory.d.ts"),
    typescript_library!("lib.es2017.string.d.ts"),
    typescript_library!("lib.es2017.typedarrays.d.ts"),
    typescript_library!("lib.es2018.d.ts"),
    typescript_library!("lib.es2018.asyncgenerator.d.ts"),
    typescript_library!("lib.es2018.asynciterable.d.ts"),
    typescript_library!("lib.es2018.promise.d.ts"),
    typescript_library!("lib.es2018.regexp.d.ts"),
    typescript_library!("lib.es2019.d.ts"),
    typescript_library!("lib.es2019.array.d.ts"),
    typescript_library!("lib.es2019.object.d.ts"),
    typescript_library!("lib.es2019.string.d.ts"),
    typescript_library!("lib.es2019.symbol.d.ts"),
    typescript_library!("lib.es2020.d.ts"),
    typescript_library!("lib.es2020.bigint.d.ts"),
    typescript_library!("lib.es2020.date.d.ts"),
    typescript_library!("lib.es2020.number.d.ts"),
    typescript_library!("lib.es2020.promise.d.ts"),
    typescript_library!("lib.es2020.sharedmemory.d.ts"),
    typescript_library!("lib.es2020.string.d.ts"),
    typescript_library!("lib.es2020.symbol.wellknown.d.ts"),
    typescript_library!("lib.es2021.d.ts"),
    typescript_library!("lib.es2021.promise.d.ts"),
    typescript_library!("lib.es2021.string.d.ts"),
    typescript_library!("lib.es2021.weakref.d.ts"),
    typescript_library!("lib.es2022.d.ts"),
    typescript_library!("lib.es2022.array.d.ts"),
    typescript_library!("lib.es2022.error.d.ts"),
    typescript_library!("lib.es2022.object.d.ts"),
    typescript_library!("lib.es2022.regexp.d.ts"),
    typescript_library!("lib.es2022.string.d.ts"),
    typescript_library!("lib.esnext.typedarrays.d.ts"),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TypeScriptScriptPhase {
    PreRequest,
    PostResponse,
}

/// An isolated JavaScript document hosted by the embedded language service.
///
/// Script and plain-snippet documents intentionally use separate projects even
/// when they target the same phase. Providers retain a version for unchanged
/// source, so sharing a slot would make one editor's request stale as soon as
/// the other editor synchronized its source.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TypeScriptDocumentKind {
    Script(TypeScriptScriptPhase),
    InteractiveConsole,
    WebSocketAutomation,
    PlainSnippet(TypeScriptScriptPhase),
    ExecutableSnippet(TypeScriptScriptPhase),
}

impl TypeScriptDocumentKind {
    const COUNT: usize = 8;

    const fn bridge_name(self) -> &'static str {
        match self {
            Self::Script(TypeScriptScriptPhase::PreRequest) => "script-pre",
            Self::Script(TypeScriptScriptPhase::PostResponse) => "script-post",
            Self::InteractiveConsole => "interactive-console",
            Self::WebSocketAutomation => "websocket-automation",
            Self::PlainSnippet(TypeScriptScriptPhase::PreRequest) => "plain-snippet-pre",
            Self::PlainSnippet(TypeScriptScriptPhase::PostResponse) => "plain-snippet-post",
            Self::ExecutableSnippet(TypeScriptScriptPhase::PreRequest) => "executable-snippet-pre",
            Self::ExecutableSnippet(TypeScriptScriptPhase::PostResponse) => {
                "executable-snippet-post"
            }
        }
    }

    const fn index(self) -> usize {
        match self {
            Self::Script(TypeScriptScriptPhase::PreRequest) => 0,
            Self::Script(TypeScriptScriptPhase::PostResponse) => 1,
            Self::InteractiveConsole => 2,
            Self::WebSocketAutomation => 7,
            Self::PlainSnippet(TypeScriptScriptPhase::PreRequest) => 3,
            Self::PlainSnippet(TypeScriptScriptPhase::PostResponse) => 4,
            Self::ExecutableSnippet(TypeScriptScriptPhase::PreRequest) => 5,
            Self::ExecutableSnippet(TypeScriptScriptPhase::PostResponse) => 6,
        }
    }

    const fn is_executable_snippet(self) -> bool {
        matches!(self, Self::ExecutableSnippet(_))
    }

    fn editor_start_utf16(self) -> usize {
        if self.is_executable_snippet() {
            GENERATOR_WRAPPER_PREFIX.encode_utf16().count()
        } else {
            0
        }
    }

    fn virtual_source<'a>(self, source: &'a str) -> Cow<'a, str> {
        if !self.is_executable_snippet() {
            return Cow::Borrowed(source);
        }

        let mut wrapped = String::with_capacity(
            GENERATOR_WRAPPER_PREFIX.len() + source.len() + GENERATOR_WRAPPER_SUFFIX.len(),
        );
        wrapped.push_str(GENERATOR_WRAPPER_PREFIX);
        wrapped.push_str(source);
        wrapped.push_str(GENERATOR_WRAPPER_SUFFIX);
        Cow::Owned(wrapped)
    }
}

impl From<TypeScriptScriptPhase> for TypeScriptDocumentKind {
    fn from(phase: TypeScriptScriptPhase) -> Self {
        Self::Script(phase)
    }
}

#[derive(Debug, Error)]
pub enum TypeScriptServiceError {
    #[error("embedded TypeScript service is unavailable: {0}")]
    Unavailable(String),
    #[error("embedded TypeScript worker stopped")]
    WorkerStopped,
    #[error("script source is {actual} bytes; the intelligence limit is {limit} bytes")]
    SourceTooLarge { actual: usize, limit: usize },
    #[error("byte offset {offset} is not a UTF-8 boundary in a {length}-byte document")]
    InvalidByteOffset { offset: usize, length: usize },
    #[error("TypeScript returned invalid UTF-16 offset {offset} for the current document")]
    InvalidUtf16Offset { offset: usize },
    #[error("document version {received} is stale; current version is {current}")]
    StaleDocument { received: u64, current: u64 },
    #[error("document version {version} was reused with different source text")]
    VersionConflict { version: u64 },
    #[error("embedded TypeScript failed: {0}")]
    Engine(String),
    #[error("embedded TypeScript returned invalid data: {0}")]
    InvalidResponse(String),
}

#[derive(Clone)]
pub struct TypeScriptServiceHandle {
    inner: Arc<ServiceInner>,
    websocket_project: Option<WebSocketScriptProject>,
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct WebSocketScriptProject {
    pub active_file: String,
    pub files: std::collections::BTreeMap<String, String>,
}

struct ServiceInner {
    sender: Sender<Command>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl Drop for ServiceInner {
    fn drop(&mut self) {
        let _ = self.sender.send(Command::Shutdown);
        if let Ok(worker) = self.worker.get_mut()
            && let Some(worker) = worker.take()
        {
            let _ = worker.join();
        }
    }
}

impl TypeScriptServiceHandle {
    /// Starts the embedded service on a dedicated thread.
    ///
    /// Parsing the official TypeScript bundle happens asynchronously on that
    /// worker. Requests sent during initialization remain queued, so creating
    /// an editor never blocks GPUI's render path.
    pub fn start() -> Result<Self, TypeScriptServiceError> {
        let (sender, receiver) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("resolved-typescript-service".to_owned())
            .spawn(move || match TypeScriptEngine::new() {
                Ok(engine) => worker_loop(engine, receiver),
                Err(error) => {
                    let message = error.to_string();
                    tracing::warn!(%message, "JavaScript language service unavailable");
                    unavailable_worker_loop(message, receiver);
                }
            })
            .map_err(|error| TypeScriptServiceError::Unavailable(error.to_string()))?;

        Ok(Self {
            websocket_project: None,
            inner: Arc::new(ServiceInner {
                sender,
                worker: Mutex::new(Some(worker)),
            }),
        })
    }

    #[cfg(test)]
    async fn sync_document(
        &self,
        document: impl Into<TypeScriptDocumentKind>,
        version: u64,
        source: String,
    ) -> Result<(), TypeScriptServiceError> {
        let document = document.into();
        self.request(|reply| Command::Sync {
            document,
            version,
            source,
            reply,
        })
        .await
    }

    pub async fn completion_items(
        &self,
        document: impl Into<TypeScriptDocumentKind>,
        version: u64,
        source: String,
        byte_offset: usize,
    ) -> Result<Vec<CompletionItem>, TypeScriptServiceError> {
        let document = document.into();
        self.request(|reply| Command::Complete {
            document,
            version,
            source,
            byte_offset,
            reply,
        })
        .await
    }

    pub async fn hover(
        &self,
        document: impl Into<TypeScriptDocumentKind>,
        version: u64,
        source: String,
        byte_offset: usize,
    ) -> Result<Option<Hover>, TypeScriptServiceError> {
        let document = document.into();
        self.request(|reply| Command::Hover {
            document,
            version,
            source,
            byte_offset,
            reply,
        })
        .await
    }

    pub async fn diagnostics(
        &self,
        document: impl Into<TypeScriptDocumentKind>,
        version: u64,
        source: String,
    ) -> Result<Vec<Diagnostic>, TypeScriptServiceError> {
        let document = document.into();
        self.request(|reply| Command::Diagnostics {
            document,
            version,
            source,
            reply,
        })
        .await
    }

    pub fn with_websocket_project(mut self, project: WebSocketScriptProject) -> Self {
        self.websocket_project = Some(project);
        self
    }

    async fn request<T>(
        &self,
        command: impl FnOnce(oneshot::Sender<Result<T, TypeScriptServiceError>>) -> Command,
    ) -> Result<T, TypeScriptServiceError>
    where
        T: Send + 'static,
    {
        let (reply, receiver) = oneshot::channel();
        let command = command(reply);
        let command = if let Some(project) = &self.websocket_project {
            Command::WebSocketProject {
                project: project.clone(),
                command: Box::new(command),
            }
        } else {
            command
        };
        self.inner
            .sender
            .send(command)
            .map_err(|_| TypeScriptServiceError::WorkerStopped)?;
        receiver
            .await
            .map_err(|_| TypeScriptServiceError::WorkerStopped)?
    }

    /// Enqueues fresh request-namespace declarations for the type checker.
    /// Fire-and-forget: the language service revalidates on its next request,
    /// which keeps script diagnostics in sync with the active workspace.
    pub fn set_request_namespace_declarations(&self, declarations: impl Into<String>) {
        let _ = self
            .inner
            .sender
            .send(Command::SetRequestNamespaceDeclarations {
                declarations: declarations.into(),
            });
    }
}

enum Command {
    WebSocketProject {
        project: WebSocketScriptProject,
        command: Box<Command>,
    },
    #[cfg(test)]
    Sync {
        document: TypeScriptDocumentKind,
        version: u64,
        source: String,
        reply: oneshot::Sender<Result<(), TypeScriptServiceError>>,
    },
    Complete {
        document: TypeScriptDocumentKind,
        version: u64,
        source: String,
        byte_offset: usize,
        reply: oneshot::Sender<Result<Vec<CompletionItem>, TypeScriptServiceError>>,
    },
    Hover {
        document: TypeScriptDocumentKind,
        version: u64,
        source: String,
        byte_offset: usize,
        reply: oneshot::Sender<Result<Option<Hover>, TypeScriptServiceError>>,
    },
    Diagnostics {
        document: TypeScriptDocumentKind,
        version: u64,
        source: String,
        reply: oneshot::Sender<Result<Vec<Diagnostic>, TypeScriptServiceError>>,
    },
    /// Fire-and-forget push of the active workspace's request-reference
    /// namespace declarations into the script/plain-snippet projects.
    SetRequestNamespaceDeclarations {
        declarations: String,
    },
    Shutdown,
}

fn worker_loop(mut engine: TypeScriptEngine, receiver: Receiver<Command>) {
    while let Ok(command) = receiver.recv() {
        let command = if let Command::WebSocketProject { project, command } = command {
            if let Err(error) = engine.set_websocket_project(&project) {
                tracing::debug!(%error, "WebSocket project unavailable");
            }
            *command
        } else {
            command
        };
        match command {
            Command::WebSocketProject { .. } => unreachable!("nested project command"),
            #[cfg(test)]
            Command::Sync {
                document,
                version,
                source,
                reply,
            } => {
                let _ = reply.send(engine.sync_document(document, version, source));
            }
            Command::Complete {
                document,
                version,
                source,
                byte_offset,
                reply,
            } => {
                let _ = reply.send(engine.completion_items(document, version, source, byte_offset));
            }
            Command::Hover {
                document,
                version,
                source,
                byte_offset,
                reply,
            } => {
                let _ = reply.send(engine.hover(document, version, source, byte_offset));
            }
            Command::Diagnostics {
                document,
                version,
                source,
                reply,
            } => {
                let _ = reply.send(engine.diagnostics(document, version, source));
            }
            Command::SetRequestNamespaceDeclarations { declarations } => {
                let _ = engine.set_request_namespace_declarations(&declarations);
            }
            Command::Shutdown => break,
        }
    }
}

fn unavailable_worker_loop(message: String, receiver: Receiver<Command>) {
    while let Ok(command) = receiver.recv() {
        let command = if let Command::WebSocketProject { command, .. } = command {
            *command
        } else {
            command
        };
        let unavailable = || TypeScriptServiceError::Unavailable(message.clone());
        match command {
            Command::WebSocketProject { .. } => unreachable!("nested project command"),
            #[cfg(test)]
            Command::Sync { reply, .. } => {
                let _ = reply.send(Err(unavailable()));
            }
            Command::Complete { reply, .. } => {
                let _ = reply.send(Err(unavailable()));
            }
            Command::Hover { reply, .. } => {
                let _ = reply.send(Err(unavailable()));
            }
            Command::Diagnostics { reply, .. } => {
                let _ = reply.send(Err(unavailable()));
            }
            Command::SetRequestNamespaceDeclarations { .. } => {}
            Command::Shutdown => break,
        }
    }
}

#[derive(Default)]
struct DocumentState {
    version: Option<u64>,
    source: String,
}

struct TypeScriptEngine {
    _runtime: Runtime,
    context: Context,
    documents: [DocumentState; TypeScriptDocumentKind::COUNT],
}

impl TypeScriptEngine {
    fn new() -> Result<Self, TypeScriptServiceError> {
        let runtime = Runtime::new()
            .map_err(|error| TypeScriptServiceError::Unavailable(error.to_string()))?;
        runtime.set_memory_limit(TYPESCRIPT_MEMORY_LIMIT_BYTES);
        runtime.set_max_stack_size(TYPESCRIPT_STACK_LIMIT_BYTES);
        let context = Context::full(&runtime)
            .map_err(|error| TypeScriptServiceError::Unavailable(error.to_string()))?;
        let libraries = serde_json::to_string(TYPESCRIPT_LIBRARIES)
            .map_err(|error| TypeScriptServiceError::Unavailable(error.to_string()))?;

        context.with(|ctx| {
            let globals = ctx.globals();
            globals
                .set("__RESOLVED_TS_LIBRARIES_JSON", libraries)
                .catch(&ctx)
                .map_err(|error| TypeScriptServiceError::Unavailable(error.to_string()))?;
            globals
                .set(
                    "__RESOLVED_TS_WEBSOCKET_DTS",
                    include_str!("typescript_service/websocket.d.ts"),
                )
                .catch(&ctx)
                .map_err(|error| TypeScriptServiceError::Unavailable(error.to_string()))?;
            globals
                .set("__RESOLVED_TS_RUNTIME_DTS", RUNTIME_DECLARATIONS)
                .catch(&ctx)
                .map_err(|error| TypeScriptServiceError::Unavailable(error.to_string()))?;
            globals
                .set("__RESOLVED_TS_PRE_DTS", PRE_REQUEST_DECLARATIONS)
                .catch(&ctx)
                .map_err(|error| TypeScriptServiceError::Unavailable(error.to_string()))?;
            globals
                .set("__RESOLVED_TS_POST_DTS", POST_RESPONSE_DECLARATIONS)
                .catch(&ctx)
                .map_err(|error| TypeScriptServiceError::Unavailable(error.to_string()))?;
            globals
                .set(
                    "__RESOLVED_TS_SNIPPET_GENERATOR_DTS",
                    SNIPPET_GENERATOR_DECLARATIONS,
                )
                .catch(&ctx)
                .map_err(|error| TypeScriptServiceError::Unavailable(error.to_string()))?;
            globals
                .set(
                    "__RESOLVED_TS_EXECUTABLE_SNIPPET_PRE_DTS",
                    EXECUTABLE_SNIPPET_PRE_DECLARATIONS,
                )
                .catch(&ctx)
                .map_err(|error| TypeScriptServiceError::Unavailable(error.to_string()))?;
            globals
                .set(
                    "__RESOLVED_TS_EXECUTABLE_SNIPPET_POST_DTS",
                    EXECUTABLE_SNIPPET_POST_DECLARATIONS,
                )
                .catch(&ctx)
                .map_err(|error| TypeScriptServiceError::Unavailable(error.to_string()))?;
            globals
                .set("__RESOLVED_TS_REQUEST_NAMESPACE_DTS", "")
                .catch(&ctx)
                .map_err(|error| TypeScriptServiceError::Unavailable(error.to_string()))?;
            ctx.eval::<(), _>(TYPESCRIPT_SOURCE)
                .catch(&ctx)
                .map_err(|error| TypeScriptServiceError::Unavailable(error.to_string()))?;
            ctx.eval::<(), _>(BRIDGE_SOURCE)
                .catch(&ctx)
                .map_err(|error| TypeScriptServiceError::Unavailable(error.to_string()))?;
            globals
                .get::<_, Object<'_>>(SERVICE_GLOBAL)
                .catch(&ctx)
                .map_err(|error| TypeScriptServiceError::Unavailable(error.to_string()))?;
            Ok(())
        })?;

        Ok(Self {
            _runtime: runtime,
            context,
            documents: std::array::from_fn(|_| DocumentState::default()),
        })
    }

    fn sync_document(
        &mut self,
        document: TypeScriptDocumentKind,
        version: u64,
        source: String,
    ) -> Result<(), TypeScriptServiceError> {
        if source.len() > MAX_DOCUMENT_BYTES {
            return Err(TypeScriptServiceError::SourceTooLarge {
                actual: source.len(),
                limit: MAX_DOCUMENT_BYTES,
            });
        }

        let state = &self.documents[document.index()];
        if let Some(current) = state.version {
            if version < current {
                return Err(TypeScriptServiceError::StaleDocument {
                    received: version,
                    current,
                });
            }
            if version == current {
                return if source == state.source {
                    Ok(())
                } else {
                    Err(TypeScriptServiceError::VersionConflict { version })
                };
            }
        }

        let virtual_source = document.virtual_source(&source);
        self.call_update_document(document, version, &virtual_source)?;
        self.documents[document.index()] = DocumentState {
            version: Some(version),
            source,
        };
        Ok(())
    }

    fn completion_items(
        &mut self,
        document: TypeScriptDocumentKind,
        version: u64,
        source: String,
        byte_offset: usize,
    ) -> Result<Vec<CompletionItem>, TypeScriptServiceError> {
        self.sync_document(document, version, source)?;
        let source = &self.documents[document.index()].source;
        let editor_utf16_offset = byte_offset_to_utf16(source, byte_offset)?;
        let utf16_offset = document
            .editor_start_utf16()
            .checked_add(editor_utf16_offset)
            .ok_or(TypeScriptServiceError::InvalidByteOffset {
                offset: byte_offset,
                length: source.len(),
            })?;
        let raw: Vec<BridgeCompletion> = self.call_json(
            "completions",
            document,
            Some(u32::try_from(utf16_offset).map_err(|_| {
                TypeScriptServiceError::InvalidByteOffset {
                    offset: byte_offset,
                    length: source.len(),
                }
            })?),
        )?;
        let mut items = Vec::with_capacity(raw.len());
        for item in raw {
            if let Some(item) = completion_item(source, document.editor_start_utf16(), item)? {
                items.push(item);
            }
        }
        Ok(items)
    }

    fn hover(
        &mut self,
        document: TypeScriptDocumentKind,
        version: u64,
        source: String,
        byte_offset: usize,
    ) -> Result<Option<Hover>, TypeScriptServiceError> {
        self.sync_document(document, version, source)?;
        let source = &self.documents[document.index()].source;
        let editor_utf16_offset = byte_offset_to_utf16(source, byte_offset)?;
        let utf16_offset = document
            .editor_start_utf16()
            .checked_add(editor_utf16_offset)
            .ok_or(TypeScriptServiceError::InvalidByteOffset {
                offset: byte_offset,
                length: source.len(),
            })?;
        let raw: Option<BridgeHover> = self.call_json(
            "hover",
            document,
            Some(u32::try_from(utf16_offset).map_err(|_| {
                TypeScriptServiceError::InvalidByteOffset {
                    offset: byte_offset,
                    length: source.len(),
                }
            })?),
        )?;
        raw.map(|hover| hover_item(source, document.editor_start_utf16(), hover))
            .transpose()
            .map(Option::flatten)
    }

    fn diagnostics(
        &mut self,
        document: TypeScriptDocumentKind,
        version: u64,
        source: String,
    ) -> Result<Vec<Diagnostic>, TypeScriptServiceError> {
        self.sync_document(document, version, source)?;
        let source = &self.documents[document.index()].source;
        let raw: Vec<BridgeDiagnostic> = self.call_json("diagnostics", document, None)?;
        raw.into_iter()
            .map(|diagnostic| diagnostic_item(source, document.editor_start_utf16(), diagnostic))
            .collect()
    }

    fn call_update_document(
        &self,
        document: TypeScriptDocumentKind,
        version: u64,
        source: &str,
    ) -> Result<(), TypeScriptServiceError> {
        self.context.with(|ctx| {
            let service: Object<'_> = ctx
                .globals()
                .get(SERVICE_GLOBAL)
                .catch(&ctx)
                .map_err(|error| TypeScriptServiceError::Engine(error.to_string()))?;
            let update: Function<'_> = service
                .get("updateDocument")
                .catch(&ctx)
                .map_err(|error| TypeScriptServiceError::Engine(error.to_string()))?;
            update
                .call::<_, ()>((document.bridge_name(), source, version.to_string()))
                .catch(&ctx)
                .map_err(|error| TypeScriptServiceError::Engine(error.to_string()))
        })
    }

    fn set_websocket_project(
        &mut self,
        project: &WebSocketScriptProject,
    ) -> Result<(), TypeScriptServiceError> {
        let json = serde_json::to_string(project)
            .map_err(|error| TypeScriptServiceError::Engine(error.to_string()))?;
        let changed = self
            .context
            .with(|ctx| {
                let service: Object<'_> = ctx.globals().get(SERVICE_GLOBAL)?;
                let setter: Function<'_> = service.get("setWebSocketProject")?;
                setter
                    .call::<_, bool>((json,))
                    .catch(&ctx)
                    .map_err(|error| {
                        rquickjs::Error::new_from_js_message(
                            "project",
                            "workspace",
                            error.to_string(),
                        )
                    })
            })
            .map_err(|error| TypeScriptServiceError::Engine(error.to_string()))?;
        if changed {
            self.documents[TypeScriptDocumentKind::WebSocketAutomation.index()] =
                DocumentState::default();
        }
        Ok(())
    }

    /// Pushes fresh request-reference namespace declarations (from the active
    /// workspace's collection tree) into the embedded language service so the
    /// type checker sees the same saved-request globals the runtime injects.
    fn set_request_namespace_declarations(
        &mut self,
        declarations: &str,
    ) -> Result<(), TypeScriptServiceError> {
        self.context.with(|ctx| {
            let service: Object<'_> = ctx
                .globals()
                .get(SERVICE_GLOBAL)
                .catch(&ctx)
                .map_err(|error| TypeScriptServiceError::Engine(error.to_string()))?;
            let setter: Function<'_> =
                service
                    .get("setRequestNamespaceDeclarations")
                    .catch(&ctx)
                    .map_err(|error| TypeScriptServiceError::Engine(error.to_string()))?;
            setter
                .call::<_, ()>((declarations,))
                .catch(&ctx)
                .map_err(|error| TypeScriptServiceError::Engine(error.to_string()))
        })
    }

    fn call_json<T>(
        &self,
        method: &str,
        document: TypeScriptDocumentKind,
        offset: Option<u32>,
    ) -> Result<T, TypeScriptServiceError>
    where
        T: for<'de> Deserialize<'de>,
    {
        let encoded = self.context.with(|ctx| {
            let service: Object<'_> = ctx
                .globals()
                .get(SERVICE_GLOBAL)
                .catch(&ctx)
                .map_err(|error| TypeScriptServiceError::Engine(error.to_string()))?;
            let function: Function<'_> = service
                .get(method)
                .catch(&ctx)
                .map_err(|error| TypeScriptServiceError::Engine(error.to_string()))?;
            let result = match offset {
                Some(offset) => function.call::<_, String>((document.bridge_name(), offset)),
                None => function.call::<_, String>((document.bridge_name(),)),
            };
            result
                .catch(&ctx)
                .map_err(|error| TypeScriptServiceError::Engine(error.to_string()))
        })?;
        serde_json::from_str(&encoded)
            .map_err(|error| TypeScriptServiceError::InvalidResponse(error.to_string()))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BridgeCompletion {
    name: String,
    kind: String,
    #[serde(default)]
    kind_modifiers: String,
    #[serde(default)]
    sort_text: Option<String>,
    #[serde(default)]
    insert_text: Option<String>,
    #[serde(default)]
    is_snippet: bool,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    replacement_span: Option<BridgeSpan>,
}

#[derive(Deserialize)]
struct BridgeSpan {
    start: usize,
    length: usize,
}

#[derive(Deserialize)]
struct BridgeHover {
    start: usize,
    length: usize,
    display: String,
    documentation: String,
}

#[derive(Deserialize)]
struct BridgeDiagnostic {
    start: usize,
    length: usize,
    category: u8,
    code: i32,
    message: String,
}

fn completion_item(
    source: &str,
    editor_start_utf16: usize,
    item: BridgeCompletion,
) -> Result<Option<CompletionItem>, TypeScriptServiceError> {
    let new_text = if item.is_snippet {
        item.name.clone()
    } else {
        item.insert_text
            .clone()
            .unwrap_or_else(|| item.name.clone())
    };
    let text_edit = if let Some(span) = item.replacement_span {
        let Some(range) =
            projected_utf16_span_to_range(source, editor_start_utf16, span.start, span.length)?
        else {
            return Ok(None);
        };
        Some(CompletionTextEdit::Edit(TextEdit {
            range,
            new_text: new_text.clone(),
        }))
    } else {
        None
    };
    let detail = [
        (!item.kind_modifiers.is_empty()).then_some(item.kind_modifiers.as_str()),
        item.source.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");

    Ok(Some(CompletionItem {
        label: item.name.clone(),
        kind: Some(completion_kind(&item.kind)),
        detail: (!detail.is_empty()).then_some(detail),
        sort_text: item.sort_text,
        filter_text: Some(item.name),
        // gpui-component treats `insert_text` without a `text_edit` as a pure
        // insertion at the cursor. Leaving it unset lets the completion menu
        // replace the typed query (for example `ma` -> `map`) using its
        // tracked trigger range.
        insert_text: None,
        text_edit,
        ..Default::default()
    }))
}

fn completion_kind(kind: &str) -> CompletionItemKind {
    match kind {
        "class" => CompletionItemKind::CLASS,
        "const" => CompletionItemKind::CONSTANT,
        "enum" => CompletionItemKind::ENUM,
        "enum member" => CompletionItemKind::ENUM_MEMBER,
        "function" => CompletionItemKind::FUNCTION,
        "getter" | "property" => CompletionItemKind::PROPERTY,
        "interface" => CompletionItemKind::INTERFACE,
        "keyword" | "primitive type" => CompletionItemKind::KEYWORD,
        "method" => CompletionItemKind::METHOD,
        "module" | "external module name" => CompletionItemKind::MODULE,
        "parameter" => CompletionItemKind::VARIABLE,
        "setter" => CompletionItemKind::PROPERTY,
        "string" => CompletionItemKind::VALUE,
        "type" => CompletionItemKind::TYPE_PARAMETER,
        "var" | "let" | "local var" | "alias" => CompletionItemKind::VARIABLE,
        _ => CompletionItemKind::TEXT,
    }
}

fn hover_item(
    source: &str,
    editor_start_utf16: usize,
    item: BridgeHover,
) -> Result<Option<Hover>, TypeScriptServiceError> {
    let Some(range) =
        projected_utf16_span_to_range(source, editor_start_utf16, item.start, item.length)?
    else {
        return Ok(None);
    };
    let value = match (item.display.is_empty(), item.documentation.is_empty()) {
        (false, false) => format!(
            "```javascript\n{}\n```\n\n{}",
            item.display, item.documentation
        ),
        (false, true) => format!("```javascript\n{}\n```", item.display),
        (true, false) => item.documentation,
        (true, true) => String::new(),
    };
    Ok(Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value,
        }),
        range: Some(range),
    }))
}

fn diagnostic_item(
    source: &str,
    editor_start_utf16: usize,
    item: BridgeDiagnostic,
) -> Result<Diagnostic, TypeScriptServiceError> {
    let severity = match item.category {
        0 => DiagnosticSeverity::WARNING,
        1 => DiagnosticSeverity::ERROR,
        2 => DiagnosticSeverity::HINT,
        _ => DiagnosticSeverity::INFORMATION,
    };
    let diagnostic_source = if item.code >= 90_000 {
        "Resolved runtime".to_owned()
    } else {
        format!("TypeScript {EMBEDDED_TYPESCRIPT_VERSION}")
    };
    Ok(Diagnostic {
        range: clamped_projected_utf16_span_to_range(
            source,
            editor_start_utf16,
            item.start,
            item.length,
        )?,
        severity: Some(severity),
        code: Some(NumberOrString::Number(item.code)),
        source: Some(diagnostic_source),
        message: item.message,
        ..Default::default()
    })
}

fn projected_utf16_span_to_range(
    source: &str,
    editor_start_utf16: usize,
    start: usize,
    length: usize,
) -> Result<Option<Range>, TypeScriptServiceError> {
    let end = start
        .checked_add(length)
        .ok_or(TypeScriptServiceError::InvalidUtf16Offset { offset: start })?;
    let editor_len_utf16 = source.encode_utf16().count();
    let editor_end_utf16 = editor_start_utf16.checked_add(editor_len_utf16).ok_or(
        TypeScriptServiceError::InvalidUtf16Offset {
            offset: editor_start_utf16,
        },
    )?;
    if start < editor_start_utf16 || end > editor_end_utf16 {
        return Ok(None);
    }
    Ok(Some(utf16_span_to_range(
        source,
        start - editor_start_utf16,
        length,
    )?))
}

fn clamped_projected_utf16_span_to_range(
    source: &str,
    editor_start_utf16: usize,
    start: usize,
    length: usize,
) -> Result<Range, TypeScriptServiceError> {
    let end = start
        .checked_add(length)
        .ok_or(TypeScriptServiceError::InvalidUtf16Offset { offset: start })?;
    let editor_len_utf16 = source.encode_utf16().count();
    let relative_start = start
        .saturating_sub(editor_start_utf16)
        .min(editor_len_utf16);
    let relative_end = end
        .saturating_sub(editor_start_utf16)
        .min(editor_len_utf16)
        .max(relative_start);
    utf16_span_to_range(source, relative_start, relative_end - relative_start)
}

fn byte_offset_to_utf16(source: &str, offset: usize) -> Result<usize, TypeScriptServiceError> {
    if offset > source.len() || !source.is_char_boundary(offset) {
        return Err(TypeScriptServiceError::InvalidByteOffset {
            offset,
            length: source.len(),
        });
    }
    Ok(source[..offset].encode_utf16().count())
}

fn utf16_offset_to_byte(source: &str, offset: usize) -> Option<usize> {
    let mut utf16_offset = 0;
    for (byte_offset, character) in source.char_indices() {
        if utf16_offset == offset {
            return Some(byte_offset);
        }
        utf16_offset += character.len_utf16();
        if utf16_offset > offset {
            return None;
        }
    }
    (utf16_offset == offset).then_some(source.len())
}

fn utf16_span_to_range(
    source: &str,
    start: usize,
    length: usize,
) -> Result<Range, TypeScriptServiceError> {
    let end = start
        .checked_add(length)
        .ok_or(TypeScriptServiceError::InvalidUtf16Offset { offset: start })?;
    Ok(Range::new(
        utf16_offset_to_position(source, start)?,
        utf16_offset_to_position(source, end)?,
    ))
}

fn utf16_offset_to_position(
    source: &str,
    offset: usize,
) -> Result<Position, TypeScriptServiceError> {
    let byte_offset = utf16_offset_to_byte(source, offset)
        .ok_or(TypeScriptServiceError::InvalidUtf16Offset { offset })?;
    let prefix = &source[..byte_offset];
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count();
    let line_start = prefix.rfind('\n').map_or(0, |index| index + 1);
    // TypeScript offsets are UTF-16 code units, but gpui-component's
    // `position_to_offset` interprets LSP columns as Unicode scalar indices.
    // Convert at this boundary so editor ranges remain correct after astral
    // characters such as emoji.
    let character = source[line_start..byte_offset].chars().count();
    Ok(Position::new(
        u32::try_from(line).map_err(|_| TypeScriptServiceError::InvalidUtf16Offset { offset })?,
        u32::try_from(character)
            .map_err(|_| TypeScriptServiceError::InvalidUtf16Offset { offset })?,
    ))
}

#[allow(dead_code)]
fn utf16_span_to_byte_range(
    source: &str,
    start: usize,
    length: usize,
) -> Result<ByteRange<usize>, TypeScriptServiceError> {
    let end = start
        .checked_add(length)
        .ok_or(TypeScriptServiceError::InvalidUtf16Offset { offset: start })?;
    let start = utf16_offset_to_byte(source, start)
        .ok_or(TypeScriptServiceError::InvalidUtf16Offset { offset: start })?;
    let end = utf16_offset_to_byte(source, end)
        .ok_or(TypeScriptServiceError::InvalidUtf16Offset { offset: end })?;
    Ok(start..end)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_async<T>(future: impl Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime")
            .block_on(future)
    }

    #[test]
    fn utf8_and_utf16_offsets_round_trip_without_splitting_surrogates() {
        let source = "a🎉中\nşehir";
        for byte_offset in 0..=source.len() {
            if !source.is_char_boundary(byte_offset) {
                assert!(matches!(
                    byte_offset_to_utf16(source, byte_offset),
                    Err(TypeScriptServiceError::InvalidByteOffset { .. })
                ));
                continue;
            }
            let utf16 = byte_offset_to_utf16(source, byte_offset).expect("valid byte boundary");
            assert_eq!(utf16_offset_to_byte(source, utf16), Some(byte_offset));
        }
        assert_eq!(utf16_offset_to_byte(source, 2), None);
        assert_eq!(
            utf16_offset_to_position(source, 3).expect("position after emoji"),
            Position::new(0, 2)
        );
        assert_eq!(
            utf16_offset_to_position(source, 5).expect("position after first line"),
            Position::new(1, 0)
        );
    }

    #[test]
    fn websocket_automation_intelligence_resolves_modules_and_runtime_api() {
        run_async(async {
            let service = TypeScriptServiceHandle::start().unwrap();
            let mut project = WebSocketScriptProject {
                active_file: "automation.js".into(),
                files: std::collections::BTreeMap::from([
                    ("automation.js".into(), "".into()),
                    ("lib/helpers.js".into(), "/** @param {number} id */ export function ack(id) { return { id, accepted: true }; }".into()),
                ]),
            };
            let configured = service.clone().with_websocket_project(project.clone());
            let source = "import { ack } from './lib/helpers.js'; const result = ack(1); result.";
            let items = configured
                .completion_items(
                    TypeScriptDocumentKind::WebSocketAutomation,
                    1,
                    source.into(),
                    source.len(),
                )
                .await
                .unwrap();
            assert!(
                items.iter().any(|item| item.label == "accepted"),
                "{items:?}"
            );
            let import_source = "import { ack } from './lib/hel";
            let imports = configured
                .completion_items(
                    TypeScriptDocumentKind::WebSocketAutomation,
                    2,
                    import_source.into(),
                    import_source.len(),
                )
                .await
                .unwrap();
            assert!(
                imports.iter().any(|item| item.label.contains("helpers")),
                "{imports:?}"
            );
            let source = "api.";
            let items = configured
                .completion_items(
                    TypeScriptDocumentKind::WebSocketAutomation,
                    3,
                    source.into(),
                    source.len(),
                )
                .await
                .unwrap();
            for name in [
                "execute",
                "requests",
                "environment",
                "request",
                "test",
                "assert",
            ] {
                assert!(
                    items.iter().any(|item| item.label == name),
                    "missing {name}: {items:?}"
                );
            }
            let diagnostics = configured.diagnostics(TypeScriptDocumentKind::WebSocketAutomation, 4,
                "api.request.headers.set('X-Test', 'yes'); api.test('ok', () => api.assert(api.response === null)); api.environment.set('key', 'value');".into()).await.unwrap();
            assert!(diagnostics.is_empty(), "{diagnostics:?}");
            let source = "ws.";
            let items = configured
                .completion_items(
                    TypeScriptDocumentKind::WebSocketAutomation,
                    5,
                    source.into(),
                    source.len(),
                )
                .await
                .unwrap();
            assert!(items.iter().any(|item| item.label == "sendJson"));
            let source = "import { ack } from './lib/helpers.js'; ws.sendJson(await Promise.resolve(ack(7))); console.log('sent');";
            let diagnostics = configured
                .diagnostics(
                    TypeScriptDocumentKind::WebSocketAutomation,
                    6,
                    source.into(),
                )
                .await
                .unwrap();
            assert!(diagnostics.is_empty(), "{diagnostics:?}");
            let source = "import { ack } from './lib/helpers.js'; ack('wrong'); ws.nonexistent(); api.request;";
            let diagnostics = configured
                .diagnostics(
                    TypeScriptDocumentKind::WebSocketAutomation,
                    7,
                    source.into(),
                )
                .await
                .unwrap();
            assert!(
                diagnostics
                    .iter()
                    .any(|item| item.message.contains("number")),
                "{diagnostics:?}"
            );
            assert!(
                diagnostics
                    .iter()
                    .any(|item| item.message.contains("nonexistent"))
            );
            assert!(
                !diagnostics
                    .iter()
                    .any(|item| item.message.contains("Cannot find name 'api'"))
            );
            let source = "import { ack } from './lib/helpers.js'; ack(2);";
            let hover = configured
                .hover(
                    TypeScriptDocumentKind::WebSocketAutomation,
                    8,
                    source.into(),
                    source.rfind("ack").unwrap(),
                )
                .await
                .unwrap();
            assert!(hover.is_some());
            project.active_file = "lib/other.js".into();
            project.files.insert("lib/other.js".into(), "".into());
            let configured = service.clone().with_websocket_project(project.clone());
            let source = "import { ack } from './helpers.js'; ws.send(ack(1));";
            assert!(
                configured
                    .diagnostics(
                        TypeScriptDocumentKind::WebSocketAutomation,
                        9,
                        source.into()
                    )
                    .await
                    .unwrap()
                    .is_empty()
            );
            project.files.remove("lib/helpers.js");
            let configured = service.with_websocket_project(project);
            let diagnostics = configured
                .diagnostics(
                    TypeScriptDocumentKind::WebSocketAutomation,
                    10,
                    source.into(),
                )
                .await
                .unwrap();
            assert!(
                diagnostics
                    .iter()
                    .any(|item| item.message.contains("Cannot find module")),
                "{diagnostics:?}"
            );
        });
    }

    #[test]
    fn real_typescript_service_provides_semantic_javascript_intelligence() {
        run_async(async {
            let service = TypeScriptServiceHandle::start().expect("embedded TypeScript starts");

            let source = "const party = \"🎉\"; const items = [party]; items.ma".to_owned();
            let completions = service
                .completion_items(
                    TypeScriptScriptPhase::PreRequest,
                    1,
                    source.clone(),
                    source.len(),
                )
                .await
                .expect("array completions");
            assert!(
                completions.iter().any(|item| item.label == "map"),
                "completion labels: {:?}",
                completions
                    .iter()
                    .map(|item| item.label.as_str())
                    .collect::<Vec<_>>()
            );
            let map = completions
                .iter()
                .find(|item| item.label == "map")
                .expect("map completion");
            assert_eq!(map.insert_text, None);
            let Some(CompletionTextEdit::Edit(edit)) = map.text_edit.as_ref() else {
                panic!("map completion must replace only the typed identifier prefix");
            };
            assert_eq!(
                edit.range.start.character,
                "const party = \"🎉\"; const items = [party]; items."
                    .chars()
                    .count() as u32
            );
            assert_eq!(edit.range.end.character, source.chars().count() as u32);
            assert_eq!(edit.new_text, "map");

            let api_source = "api.resp".to_owned();
            let pre = service
                .completion_items(
                    TypeScriptScriptPhase::PreRequest,
                    2,
                    api_source.clone(),
                    api_source.len(),
                )
                .await
                .expect("pre-request API completions");
            let post = service
                .completion_items(
                    TypeScriptScriptPhase::PostResponse,
                    1,
                    api_source.clone(),
                    api_source.len(),
                )
                .await
                .expect("post-response API completions");
            assert!(pre.iter().any(|item| item.label == "response"));
            assert!(post.iter().any(|item| item.label == "response"));

            let diagnostic_source =
                "const marker = \"🎉\"; const value = 1; value.toUpperCase();".to_owned();
            let diagnostics = service
                .diagnostics(
                    TypeScriptScriptPhase::PreRequest,
                    3,
                    diagnostic_source.clone(),
                )
                .await
                .expect("semantic diagnostics");
            assert!(diagnostics.iter().any(|diagnostic| {
                diagnostic.message.contains("toUpperCase")
                    && diagnostic.severity == Some(DiagnosticSeverity::ERROR)
            }));

            let host_source = concat!(
                "atob('YQ=='); btoa('a'); performance.now(); ",
                "new DOMException('bad'); Uint8Array.fromHex('ff'); ",
                "Intl.Collator(); Promise.resolve().then(() => {}); ",
                "async function late() {}",
            )
            .to_owned();
            let host_diagnostics = service
                .diagnostics(TypeScriptScriptPhase::PreRequest, 4, host_source)
                .await
                .expect("QuickJS host diagnostics");
            assert_eq!(
                host_diagnostics
                    .iter()
                    .filter(|diagnostic| diagnostic.message.contains("atob")
                        || diagnostic.message.contains("btoa")
                        || diagnostic.message.contains("performance")
                        || diagnostic.message.contains("DOMException")
                        || diagnostic.message.contains("fromHex"))
                    .count(),
                0,
                "real QuickJS globals must be declared: {host_diagnostics:?}"
            );
            assert!(
                host_diagnostics.iter().any(|diagnostic| {
                    diagnostic.code == Some(NumberOrString::Number(90001))
                        && diagnostic.severity == Some(DiagnosticSeverity::ERROR)
                        && diagnostic.message.contains("Intl")
                }),
                "missing Intl runtime diagnostic: {host_diagnostics:?}"
            );
            assert!(
                host_diagnostics.iter().any(|diagnostic| {
                    diagnostic.code == Some(NumberOrString::Number(90002))
                        && diagnostic.severity == Some(DiagnosticSeverity::WARNING)
                        && diagnostic.message.contains("Promise jobs")
                }),
                "missing Promise runtime diagnostic: {host_diagnostics:?}"
            );

            let shadowed_host_source = concat!(
                "function useIntl(Intl) { return Intl.Collator(); } ",
                "function usePromise(Promise) { return Promise; } ",
                "function useGlobalThis(globalThis) { return globalThis.Intl; } ",
                "useIntl({ Collator() { return 'local'; } }); ",
                "usePromise(1); useGlobalThis({ Intl: 1 });",
            )
            .to_owned();
            let shadowed_host_diagnostics = service
                .diagnostics(TypeScriptScriptPhase::PreRequest, 5, shadowed_host_source)
                .await
                .expect("locally shadowed host-global diagnostics");
            assert!(
                shadowed_host_diagnostics.iter().all(|diagnostic| {
                    !matches!(diagnostic.code, Some(NumberOrString::Number(90001 | 90002)))
                }),
                "local shadows must not receive runtime-global diagnostics: {shadowed_host_diagnostics:?}"
            );

            let hover_source = "const items = [1]; items.map".to_owned();
            let hover = service
                .hover(
                    TypeScriptScriptPhase::PostResponse,
                    2,
                    hover_source.clone(),
                    hover_source.len(),
                )
                .await
                .expect("hover request")
                .expect("map hover");
            let HoverContents::Markup(markup) = hover.contents else {
                panic!("expected markup hover");
            };
            assert!(markup.value.contains("map"));

            let post_contract_diagnostics = service
                .diagnostics(
                    TypeScriptScriptPhase::PostResponse,
                    4,
                    concat!(
                        "console.log = () => {}; ",
                        "api.test('async', async () => {});",
                    )
                    .to_owned(),
                )
                .await
                .expect("post-response runtime contract diagnostics");
            assert!(post_contract_diagnostics.iter().any(|diagnostic| {
                diagnostic.message.contains("read-only") && diagnostic.message.contains("log")
            }));
            assert!(post_contract_diagnostics.iter().any(|diagnostic| {
                diagnostic.severity == Some(DiagnosticSeverity::ERROR)
                    && diagnostic.message.contains("argument")
            }));

            let stale = service
                .sync_document(
                    TypeScriptScriptPhase::PreRequest,
                    4,
                    "const stale = true;".to_owned(),
                )
                .await
                .expect_err("older document version must be rejected");
            assert!(matches!(
                stale,
                TypeScriptServiceError::StaleDocument {
                    received: 4,
                    current: 5
                }
            ));
        });
    }

    #[test]
    fn snippet_documents_are_isolated_typed_and_projected_to_visible_source() {
        run_async(async {
            let service = TypeScriptServiceHandle::start().expect("embedded TypeScript starts");
            let pre = TypeScriptScriptPhase::PreRequest;
            let post = TypeScriptScriptPhase::PostResponse;

            // The real script editor and a plain snippet targeting the same
            // phase must not overwrite each other's retained document state.
            let script_source = "const values = [1]; values.ma".to_owned();
            let script_items = service
                .completion_items(
                    TypeScriptDocumentKind::Script(pre),
                    10,
                    script_source.clone(),
                    script_source.len(),
                )
                .await
                .expect("script completions");
            assert!(script_items.iter().any(|item| item.label == "map"));

            let plain_source = "const values = [1]; values.fi".to_owned();
            let plain_items = service
                .completion_items(
                    TypeScriptDocumentKind::PlainSnippet(pre),
                    1,
                    plain_source.clone(),
                    plain_source.len(),
                )
                .await
                .expect("plain-snippet completions");
            assert!(plain_items.iter().any(|item| item.label == "filter"));

            let original_script_items = service
                .completion_items(
                    TypeScriptDocumentKind::Script(pre),
                    10,
                    script_source.clone(),
                    script_source.len(),
                )
                .await
                .expect("unchanged script remains current");
            assert!(original_script_items.iter().any(|item| item.label == "map"));

            let script_post_source = "api.te".to_owned();
            let script_post_items = service
                .completion_items(
                    TypeScriptDocumentKind::Script(post),
                    1,
                    script_post_source.clone(),
                    script_post_source.len(),
                )
                .await
                .expect("post-response script completions");
            assert!(script_post_items.iter().any(|item| item.label == "test"));

            let pre_api_source = "api.te".to_owned();
            let post_api_source = pre_api_source.clone();
            let plain_pre_api = service
                .completion_items(
                    TypeScriptDocumentKind::PlainSnippet(pre),
                    2,
                    pre_api_source.clone(),
                    pre_api_source.len(),
                )
                .await
                .expect("plain pre-request API completions");
            let plain_post_api = service
                .completion_items(
                    TypeScriptDocumentKind::PlainSnippet(post),
                    1,
                    post_api_source.clone(),
                    post_api_source.len(),
                )
                .await
                .expect("plain post-response API completions");
            assert!(!plain_pre_api.iter().any(|item| item.label == "test"));
            assert!(plain_post_api.iter().any(|item| item.label == "test"));

            let generator_source =
                "const marker = \"🎉\";\nreturn snippet.selection?.te".to_owned();
            let generator_items = service
                .completion_items(
                    TypeScriptDocumentKind::ExecutableSnippet(post),
                    1,
                    generator_source.clone(),
                    generator_source.len(),
                )
                .await
                .expect("generator completions");
            let text = generator_items
                .iter()
                .find(|item| item.label == "text")
                .expect("selection text completion");
            let Some(CompletionTextEdit::Edit(edit)) = text.text_edit.as_ref() else {
                panic!("generator completion must have a projected edit");
            };
            assert_eq!(
                edit.range,
                Range::new(Position::new(1, 26), Position::new(1, 28)),
                "wrapper prefix must not leak into editor coordinates"
            );

            let pre_response_source = "return api.res".to_owned();
            let pre_response_items = service
                .completion_items(
                    TypeScriptDocumentKind::ExecutableSnippet(pre),
                    1,
                    pre_response_source.clone(),
                    pre_response_source.len(),
                )
                .await
                .expect("pre-request generator API completions");
            assert!(
                !pre_response_items
                    .iter()
                    .any(|item| item.label == "response")
            );

            let post_response_source = "return api.response?.st".to_owned();
            let post_response_items = service
                .completion_items(
                    TypeScriptDocumentKind::ExecutableSnippet(post),
                    2,
                    post_response_source.clone(),
                    post_response_source.len(),
                )
                .await
                .expect("post-response generator API completions");
            assert!(
                post_response_items
                    .iter()
                    .any(|item| item.label == "status")
            );

            let nullable_response_source = "return api.response".to_owned();
            let nullable_response_hover = service
                .hover(
                    TypeScriptDocumentKind::ExecutableSnippet(post),
                    3,
                    nullable_response_source.clone(),
                    nullable_response_source.len(),
                )
                .await
                .expect("nullable response hover")
                .expect("response hover");
            let HoverContents::Markup(nullable_response_markup) = nullable_response_hover.contents
            else {
                panic!("expected nullable response markup");
            };
            assert!(nullable_response_markup.value.contains("null"));

            let unsafe_response_source = "return api.response.status;".to_owned();
            let unsafe_response_diagnostics = service
                .diagnostics(
                    TypeScriptDocumentKind::ExecutableSnippet(post),
                    4,
                    unsafe_response_source,
                )
                .await
                .expect("unsafe nullable response diagnostics");
            assert!(unsafe_response_diagnostics.iter().any(|diagnostic| {
                diagnostic.message.contains("possibly 'null'")
                    && diagnostic.severity == Some(DiagnosticSeverity::ERROR)
            }));

            let safe_response_source = "return api.response?.status;".to_owned();
            let safe_response_diagnostics = service
                .diagnostics(
                    TypeScriptDocumentKind::ExecutableSnippet(post),
                    5,
                    safe_response_source,
                )
                .await
                .expect("safe nullable response diagnostics");
            assert!(
                safe_response_diagnostics
                    .iter()
                    .all(|diagnostic| !diagnostic.message.contains("possibly 'null'"))
            );

            let unsafe_selection_source = "return snippet.selection.text;".to_owned();
            let unsafe_selection_diagnostics = service
                .diagnostics(
                    TypeScriptDocumentKind::ExecutableSnippet(post),
                    6,
                    unsafe_selection_source,
                )
                .await
                .expect("unsafe nullable selection diagnostics");
            assert!(unsafe_selection_diagnostics.iter().any(|diagnostic| {
                diagnostic.message.contains("possibly 'null'")
                    && diagnostic.severity == Some(DiagnosticSeverity::ERROR)
            }));

            let safe_selection_source = "return snippet.selection?.text;".to_owned();
            let safe_selection_diagnostics = service
                .diagnostics(
                    TypeScriptDocumentKind::ExecutableSnippet(post),
                    7,
                    safe_selection_source,
                )
                .await
                .expect("safe nullable selection diagnostics");
            assert!(
                safe_selection_diagnostics
                    .iter()
                    .all(|diagnostic| !diagnostic.message.contains("possibly 'null'"))
            );

            let valid_return = "const marker = '🎉';\nreturn marker;".to_owned();
            let valid_diagnostics = service
                .diagnostics(
                    TypeScriptDocumentKind::ExecutableSnippet(post),
                    8,
                    valid_return,
                )
                .await
                .expect("valid generator diagnostics");
            assert!(
                valid_diagnostics.is_empty(),
                "function-body return should be valid: {valid_diagnostics:?}"
            );

            let invalid_source = "const value = 1;\nvalue.toUpperCase();\nreturn '';".to_owned();
            let invalid_diagnostics = service
                .diagnostics(
                    TypeScriptDocumentKind::ExecutableSnippet(post),
                    9,
                    invalid_source,
                )
                .await
                .expect("invalid generator diagnostics");
            let invalid_member = invalid_diagnostics
                .iter()
                .find(|diagnostic| diagnostic.message.contains("toUpperCase"))
                .expect("ordinary JavaScript semantic diagnostic");
            assert_eq!(invalid_member.range.start.line, 1);

            for (version, source) in [
                (10, "snippet.write('ok');"),
                (11, "return 'ok';"),
                (12, "return snippet.result('ok');"),
                (13, "return { text: 'structural result' };"),
            ] {
                let diagnostics = service
                    .diagnostics(
                        TypeScriptDocumentKind::ExecutableSnippet(post),
                        version,
                        source.to_owned(),
                    )
                    .await
                    .expect("valid generator return diagnostics");
                assert!(
                    diagnostics.is_empty(),
                    "valid generator source should be clean: {source:?}: {diagnostics:?}"
                );
            }

            for (version, source) in [(14, "return 42;"), (15, "return Promise.resolve('later');")]
            {
                let diagnostics = service
                    .diagnostics(
                        TypeScriptDocumentKind::ExecutableSnippet(post),
                        version,
                        source.to_owned(),
                    )
                    .await
                    .expect("invalid generator return diagnostics");
                assert!(
                    diagnostics.iter().any(|diagnostic| {
                        diagnostic.severity == Some(DiagnosticSeverity::ERROR)
                            && diagnostic.message.contains("not assignable")
                    }),
                    "invalid generator return should be rejected: {source:?}: {diagnostics:?}"
                );
            }

            let incomplete_source = "return {".to_owned();
            let incomplete_diagnostics = service
                .diagnostics(
                    TypeScriptDocumentKind::ExecutableSnippet(post),
                    16,
                    incomplete_source.clone(),
                )
                .await
                .expect("incomplete generator diagnostics");
            assert!(!incomplete_diagnostics.is_empty());
            assert!(incomplete_diagnostics.iter().all(|diagnostic| {
                diagnostic.range.start.line == 0
                    && diagnostic.range.end.line == 0
                    && diagnostic.range.start.character <= incomplete_source.chars().count() as u32
                    && diagnostic.range.end.character <= incomplete_source.chars().count() as u32
            }));
        });
    }

    #[test]
    fn request_namespace_declarations_suppress_unknown_name_errors() {
        run_async(async {
            let service = TypeScriptServiceHandle::start().expect("embedded TypeScript starts");

            // Build the active-workspace request namespace (ChatAdmin/Login).
            use crate::core::{RequestDraft, RequestScripts, RequestTemplate};
            let mut workspace = crate::core::Workspace::default();
            let chat = workspace.create_collection("ChatAdmin").unwrap();
            workspace
                .create_saved_request(
                    &chat,
                    "Login",
                    RequestTemplate {
                        request: RequestDraft::new("POST", "https://a.test/login"),
                        scripts: RequestScripts::default(),
                        documentation: String::new(),
                        websocket: None,
                    },
                )
                .unwrap();
            let catalog = crate::core::RequestNamespaceCatalog::from_workspace(&workspace);
            service.set_request_namespace_declarations(catalog.declaration_source());

            // With the declarations pushed, a valid reference and api.requests
            // must not be reported as unknown by the real checker.
            let clean = service
                .diagnostics(
                    TypeScriptScriptPhase::PreRequest,
                    1,
                    "api.requests.execute(ChatAdmin.Login);".to_owned(),
                )
                .await
                .expect("valid-reference diagnostics");
            assert!(
                !clean
                    .iter()
                    .any(|d| d.message.contains("cannot find name 'ChatAdmin'")),
                "a declared collection root must not be unknown: {clean:?}"
            );
            assert!(
                !clean.iter().any(|d| d.message.contains("'requests'")),
                "api.requests must be declared on the api type: {clean:?}"
            );

            // A collection root absent from the active workspace is still
            // flagged, so the checker and the Resolved stale-reference
            // diagnostic agree.
            let stale = service
                .diagnostics(
                    TypeScriptScriptPhase::PreRequest,
                    2,
                    "api.requests.execute(Nope.Thing);".to_owned(),
                )
                .await
                .expect("stale-reference diagnostics");
            assert!(
                stale
                    .iter()
                    .any(|d| d.message.contains("Cannot find name 'Nope'")),
                "an unknown collection root should still be flagged: {stale:?}"
            );
        });
    }

    #[test]
    fn top_level_await_is_not_flagged_by_the_checker() {
        run_async(async {
            let service = TypeScriptServiceHandle::start().expect("embedded TypeScript starts");
            let diagnostics = service
                .diagnostics(
                    TypeScriptScriptPhase::PreRequest,
                    1,
                    "await Promise.resolve(); api.environment.set(\"k\", \"v\");".to_owned(),
                )
                .await
                .expect("top-level-await diagnostics");
            assert!(
                !diagnostics
                    .iter()
                    .any(|d| d.message.contains("Top-level 'await'")),
                "top-level await must type-check against the runtime's async support: {diagnostics:?}"
            );
        });
    }
}

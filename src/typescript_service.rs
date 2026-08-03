//! Embedded JavaScript intelligence backed by Microsoft's TypeScript service.
//!
//! This module is deliberately a thin, in-memory host around the official
//! TypeScript LanguageService. It does not implement JavaScript parsing, type
//! inference, or the Language Server Protocol. The TypeScript engine runs in a
//! dedicated QuickJS runtime on its own thread and never shares state with the
//! sandbox used to execute request scripts.

use std::{
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

impl TypeScriptScriptPhase {
    const fn bridge_name(self) -> &'static str {
        match self {
            Self::PreRequest => "pre",
            Self::PostResponse => "post",
        }
    }

    const fn index(self) -> usize {
        match self {
            Self::PreRequest => 0,
            Self::PostResponse => 1,
        }
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
            inner: Arc::new(ServiceInner {
                sender,
                worker: Mutex::new(Some(worker)),
            }),
        })
    }

    #[cfg(test)]
    async fn sync_document(
        &self,
        phase: TypeScriptScriptPhase,
        version: u64,
        source: String,
    ) -> Result<(), TypeScriptServiceError> {
        self.request(|reply| Command::Sync {
            phase,
            version,
            source,
            reply,
        })
        .await
    }

    pub async fn completion_items(
        &self,
        phase: TypeScriptScriptPhase,
        version: u64,
        source: String,
        byte_offset: usize,
    ) -> Result<Vec<CompletionItem>, TypeScriptServiceError> {
        self.request(|reply| Command::Complete {
            phase,
            version,
            source,
            byte_offset,
            reply,
        })
        .await
    }

    pub async fn hover(
        &self,
        phase: TypeScriptScriptPhase,
        version: u64,
        source: String,
        byte_offset: usize,
    ) -> Result<Option<Hover>, TypeScriptServiceError> {
        self.request(|reply| Command::Hover {
            phase,
            version,
            source,
            byte_offset,
            reply,
        })
        .await
    }

    pub async fn diagnostics(
        &self,
        phase: TypeScriptScriptPhase,
        version: u64,
        source: String,
    ) -> Result<Vec<Diagnostic>, TypeScriptServiceError> {
        self.request(|reply| Command::Diagnostics {
            phase,
            version,
            source,
            reply,
        })
        .await
    }

    async fn request<T>(
        &self,
        command: impl FnOnce(oneshot::Sender<Result<T, TypeScriptServiceError>>) -> Command,
    ) -> Result<T, TypeScriptServiceError>
    where
        T: Send + 'static,
    {
        let (reply, receiver) = oneshot::channel();
        self.inner
            .sender
            .send(command(reply))
            .map_err(|_| TypeScriptServiceError::WorkerStopped)?;
        receiver
            .await
            .map_err(|_| TypeScriptServiceError::WorkerStopped)?
    }
}

enum Command {
    #[cfg(test)]
    Sync {
        phase: TypeScriptScriptPhase,
        version: u64,
        source: String,
        reply: oneshot::Sender<Result<(), TypeScriptServiceError>>,
    },
    Complete {
        phase: TypeScriptScriptPhase,
        version: u64,
        source: String,
        byte_offset: usize,
        reply: oneshot::Sender<Result<Vec<CompletionItem>, TypeScriptServiceError>>,
    },
    Hover {
        phase: TypeScriptScriptPhase,
        version: u64,
        source: String,
        byte_offset: usize,
        reply: oneshot::Sender<Result<Option<Hover>, TypeScriptServiceError>>,
    },
    Diagnostics {
        phase: TypeScriptScriptPhase,
        version: u64,
        source: String,
        reply: oneshot::Sender<Result<Vec<Diagnostic>, TypeScriptServiceError>>,
    },
    Shutdown,
}

fn worker_loop(mut engine: TypeScriptEngine, receiver: Receiver<Command>) {
    while let Ok(command) = receiver.recv() {
        match command {
            #[cfg(test)]
            Command::Sync {
                phase,
                version,
                source,
                reply,
            } => {
                let _ = reply.send(engine.sync_document(phase, version, source));
            }
            Command::Complete {
                phase,
                version,
                source,
                byte_offset,
                reply,
            } => {
                let _ = reply.send(engine.completion_items(phase, version, source, byte_offset));
            }
            Command::Hover {
                phase,
                version,
                source,
                byte_offset,
                reply,
            } => {
                let _ = reply.send(engine.hover(phase, version, source, byte_offset));
            }
            Command::Diagnostics {
                phase,
                version,
                source,
                reply,
            } => {
                let _ = reply.send(engine.diagnostics(phase, version, source));
            }
            Command::Shutdown => break,
        }
    }
}

fn unavailable_worker_loop(message: String, receiver: Receiver<Command>) {
    while let Ok(command) = receiver.recv() {
        let unavailable = || TypeScriptServiceError::Unavailable(message.clone());
        match command {
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
    documents: [DocumentState; 2],
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
            documents: [DocumentState::default(), DocumentState::default()],
        })
    }

    fn sync_document(
        &mut self,
        phase: TypeScriptScriptPhase,
        version: u64,
        source: String,
    ) -> Result<(), TypeScriptServiceError> {
        if source.len() > MAX_DOCUMENT_BYTES {
            return Err(TypeScriptServiceError::SourceTooLarge {
                actual: source.len(),
                limit: MAX_DOCUMENT_BYTES,
            });
        }

        let state = &self.documents[phase.index()];
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

        self.call_update_document(phase, version, &source)?;
        self.documents[phase.index()] = DocumentState {
            version: Some(version),
            source,
        };
        Ok(())
    }

    fn completion_items(
        &mut self,
        phase: TypeScriptScriptPhase,
        version: u64,
        source: String,
        byte_offset: usize,
    ) -> Result<Vec<CompletionItem>, TypeScriptServiceError> {
        self.sync_document(phase, version, source)?;
        let source = &self.documents[phase.index()].source;
        let utf16_offset = byte_offset_to_utf16(source, byte_offset)?;
        let raw: Vec<BridgeCompletion> = self.call_json(
            "completions",
            phase,
            Some(u32::try_from(utf16_offset).map_err(|_| {
                TypeScriptServiceError::InvalidByteOffset {
                    offset: byte_offset,
                    length: source.len(),
                }
            })?),
        )?;
        raw.into_iter()
            .map(|item| completion_item(source, item))
            .collect()
    }

    fn hover(
        &mut self,
        phase: TypeScriptScriptPhase,
        version: u64,
        source: String,
        byte_offset: usize,
    ) -> Result<Option<Hover>, TypeScriptServiceError> {
        self.sync_document(phase, version, source)?;
        let source = &self.documents[phase.index()].source;
        let utf16_offset = byte_offset_to_utf16(source, byte_offset)?;
        let raw: Option<BridgeHover> = self.call_json(
            "hover",
            phase,
            Some(u32::try_from(utf16_offset).map_err(|_| {
                TypeScriptServiceError::InvalidByteOffset {
                    offset: byte_offset,
                    length: source.len(),
                }
            })?),
        )?;
        raw.map(|hover| hover_item(source, hover)).transpose()
    }

    fn diagnostics(
        &mut self,
        phase: TypeScriptScriptPhase,
        version: u64,
        source: String,
    ) -> Result<Vec<Diagnostic>, TypeScriptServiceError> {
        self.sync_document(phase, version, source)?;
        let source = &self.documents[phase.index()].source;
        let raw: Vec<BridgeDiagnostic> = self.call_json("diagnostics", phase, None)?;
        raw.into_iter()
            .map(|diagnostic| diagnostic_item(source, diagnostic))
            .collect()
    }

    fn call_update_document(
        &self,
        phase: TypeScriptScriptPhase,
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
                .call::<_, ()>((phase.bridge_name(), source, version.to_string()))
                .catch(&ctx)
                .map_err(|error| TypeScriptServiceError::Engine(error.to_string()))
        })
    }

    fn call_json<T>(
        &self,
        method: &str,
        phase: TypeScriptScriptPhase,
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
                Some(offset) => function.call::<_, String>((phase.bridge_name(), offset)),
                None => function.call::<_, String>((phase.bridge_name(),)),
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
    item: BridgeCompletion,
) -> Result<CompletionItem, TypeScriptServiceError> {
    let new_text = if item.is_snippet {
        item.name.clone()
    } else {
        item.insert_text
            .clone()
            .unwrap_or_else(|| item.name.clone())
    };
    let text_edit = item
        .replacement_span
        .map(|span| {
            Ok(CompletionTextEdit::Edit(TextEdit {
                range: utf16_span_to_range(source, span.start, span.length)?,
                new_text: new_text.clone(),
            }))
        })
        .transpose()?;
    let detail = [
        (!item.kind_modifiers.is_empty()).then_some(item.kind_modifiers.as_str()),
        item.source.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");

    Ok(CompletionItem {
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
    })
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

fn hover_item(source: &str, item: BridgeHover) -> Result<Hover, TypeScriptServiceError> {
    let value = match (item.display.is_empty(), item.documentation.is_empty()) {
        (false, false) => format!(
            "```javascript\n{}\n```\n\n{}",
            item.display, item.documentation
        ),
        (false, true) => format!("```javascript\n{}\n```", item.display),
        (true, false) => item.documentation,
        (true, true) => String::new(),
    };
    Ok(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value,
        }),
        range: Some(utf16_span_to_range(source, item.start, item.length)?),
    })
}

fn diagnostic_item(
    source: &str,
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
        range: utf16_span_to_range(source, item.start, item.length)?,
        severity: Some(severity),
        code: Some(NumberOrString::Number(item.code)),
        source: Some(diagnostic_source),
        message: item.message,
        ..Default::default()
    })
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
}

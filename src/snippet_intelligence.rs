//! Editor intelligence for JavaScript snippets.
//!
//! Every snippet targets either the pre-request or post-response script phase.
//! Plain snippets use the existing request-script provider because their source
//! is inserted verbatim. Executable snippets run in a different, read-only
//! generator runtime; this module models its exact `api` and `snippet` globals
//! without leaking mutable request-script capabilities into generators.

#![allow(dead_code)]

use std::borrow::Cow;

use anyhow::Result;
use gpui::{App, Context, Task, Window};
use gpui_component::input::{CompletionProvider, HoverProvider, InputState, Rope};
use lsp_types::{
    CompletionContext, CompletionItem, CompletionItemKind, CompletionResponse, CompletionTextEdit,
    Documentation, Hover, HoverContents, MarkupContent, MarkupKind, Position, Range, TextEdit,
};

use crate::{
    core::{GENERATOR_WRAPPER_PREFIX, GENERATOR_WRAPPER_SUFFIX},
    script_intelligence::ScriptEditorPhase,
};

/// Script phase into which a generated or literal snippet will be inserted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SnippetTargetPhase {
    PreRequest,
    PostResponse,
}

impl SnippetTargetPhase {
    pub const fn script_editor_phase(self) -> ScriptEditorPhase {
        match self {
            Self::PreRequest => ScriptEditorPhase::PreRequest,
            Self::PostResponse => ScriptEditorPhase::PostResponse,
        }
    }
}

/// The JavaScript environment represented by a snippet editor.
///
/// Keeping this distinct from the pre-request/post-response script phase lets
/// the TypeScript-backed editor service add snippet documents without making
/// the snippet workspace depend on request-script implementation details.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SnippetEditorContext {
    /// Literal JavaScript copied exactly as written.
    PlainJavaScript(SnippetTargetPhase),
    /// JavaScript evaluated by Resolved to generate the copied snippet.
    ExecutableGenerator(SnippetTargetPhase),
}

impl SnippetEditorContext {
    pub const fn phase(self) -> SnippetTargetPhase {
        match self {
            Self::PlainJavaScript(phase) | Self::ExecutableGenerator(phase) => phase,
        }
    }

    pub const fn is_executable(self) -> bool {
        matches!(self, Self::ExecutableGenerator(_))
    }

    /// Ambient declarations to add to the future TypeScript-service document.
    ///
    /// Plain source sees the execution API of its target editor. Executable
    /// source instead sees the phase-appropriate read-only generator snapshot
    /// plus a second virtual declaration file for `snippet`. Keeping these
    /// files separate lets the TypeScript service reuse its shared `Resolved`
    /// runtime declarations.
    pub const fn ambient_declarations(self) -> SnippetAmbientDeclarations {
        let phase = match self {
            Self::PlainJavaScript(SnippetTargetPhase::PreRequest) => PRE_REQUEST_SNIPPET_GLOBALS,
            Self::ExecutableGenerator(SnippetTargetPhase::PreRequest) => {
                EXECUTABLE_PRE_REQUEST_SNIPPET_GLOBALS
            }
            Self::PlainJavaScript(SnippetTargetPhase::PostResponse) => {
                POST_RESPONSE_SNIPPET_GLOBALS
            }
            Self::ExecutableGenerator(SnippetTargetPhase::PostResponse) => {
                EXECUTABLE_POST_RESPONSE_SNIPPET_GLOBALS
            }
        };
        SnippetAmbientDeclarations {
            phase,
            generator: if self.is_executable() {
                Some(SNIPPET_GENERATOR_DECLARATIONS)
            } else {
                None
            },
        }
    }

    /// Builds the document presented to a JavaScript/TypeScript language
    /// service while retaining an exact UTF-16 mapping back to the visible
    /// editor source.
    ///
    /// Executable snippets are function bodies at runtime, so their top-level
    /// `return` statements are only valid after wrapping. Plain snippets are
    /// already complete target-script documents and remain unwrapped.
    pub fn virtual_document<'a>(self, editor_source: &'a str) -> SnippetVirtualDocument<'a> {
        if !self.is_executable() {
            return SnippetVirtualDocument {
                source: Cow::Borrowed(editor_source),
                editor_start_utf16: 0,
                editor_len_utf16: editor_source.encode_utf16().count(),
            };
        }
        let mut source = String::with_capacity(
            GENERATOR_WRAPPER_PREFIX.len() + editor_source.len() + GENERATOR_WRAPPER_SUFFIX.len(),
        );
        source.push_str(GENERATOR_WRAPPER_PREFIX);
        source.push_str(editor_source);
        source.push_str(GENERATOR_WRAPPER_SUFFIX);
        SnippetVirtualDocument {
            source: Cow::Owned(source),
            editor_start_utf16: GENERATOR_WRAPPER_PREFIX.encode_utf16().count(),
            editor_len_utf16: editor_source.encode_utf16().count(),
        }
    }
}

/// Source and offset mapping for one future TypeScript-service document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnippetVirtualDocument<'a> {
    source: Cow<'a, str>,
    editor_start_utf16: usize,
    editor_len_utf16: usize,
}

impl<'a> SnippetVirtualDocument<'a> {
    pub fn source(&self) -> &str {
        &self.source
    }

    pub const fn editor_start_utf16(&self) -> usize {
        self.editor_start_utf16
    }

    pub const fn editor_len_utf16(&self) -> usize {
        self.editor_len_utf16
    }

    pub fn editor_to_virtual_utf16(&self, editor_offset: usize) -> Option<usize> {
        (editor_offset <= self.editor_len_utf16).then_some(self.editor_start_utf16 + editor_offset)
    }

    pub fn virtual_to_editor_utf16(&self, virtual_offset: usize) -> Option<usize> {
        let editor_offset = virtual_offset.checked_sub(self.editor_start_utf16)?;
        (editor_offset <= self.editor_len_utf16).then_some(editor_offset)
    }
}

/// Virtual declaration files required by one snippet editor context.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SnippetAmbientDeclarations {
    phase: &'static str,
    generator: Option<&'static str>,
}

impl SnippetAmbientDeclarations {
    pub const fn phase(self) -> &'static str {
        self.phase
    }

    pub const fn generator(self) -> Option<&'static str> {
        self.generator
    }

    /// Declaration sources in deterministic virtual-file order.
    pub fn sources(self) -> impl Iterator<Item = &'static str> {
        std::iter::once(self.phase).chain(self.generator)
    }
}

pub const PRE_REQUEST_SNIPPET_GLOBALS: &str = r#"
declare const api: Resolved.PreRequestApi;
declare const console: Resolved.ScriptConsole;
"#;

pub const POST_RESPONSE_SNIPPET_GLOBALS: &str = r#"
declare const api: Resolved.PostResponseApi;
declare const console: Resolved.ScriptConsole;
"#;

pub const EXECUTABLE_PRE_REQUEST_SNIPPET_GLOBALS: &str = r#"
declare const api: ResolvedSnippet.PreRequestGeneratorApi;
declare const console: Resolved.ScriptConsole;
"#;

/// Executable generators may be invoked from the post-response snippet
/// workspace before any HTTP response exists. This differs from an inserted
/// post-response script, where `api.response` is always present at execution.
pub const EXECUTABLE_POST_RESPONSE_SNIPPET_GLOBALS: &str = r#"
declare const api: ResolvedSnippet.PostResponseGeneratorApi;
declare const console: Resolved.ScriptConsole;
"#;

/// TypeScript declarations for the JavaScript API preloaded into executable
/// snippet generators.
///
/// The declaration is intentionally standalone so the embedded TypeScript
/// service can install it as a virtual `.d.ts` file. Runtime code must expose
/// the same surface. `write` is retained as a compatibility alias for the
/// original executable-snippet sketch; new snippets should prefer
/// `snippet.write`.
pub const SNIPPET_GENERATOR_DECLARATIONS: &str = r#"
declare namespace ResolvedSnippet {
  type Category = "pre-request" | "post-response";
  type SelectionSource = "pre-request" | "post-response" | "request" | "response";
  type SelectionArea = "script" | "url" | "headers" | "body";

  interface Request {
    readonly method: Resolved.HttpMethod;
    readonly url: string;
    readonly body: string;
    readonly bodyMode: Resolved.BodyMode;
    readonly rawBodyLanguage: Resolved.RawBodyLanguage;
    readonly bodyFields: readonly Resolved.ReadonlyBodyField[];
    readonly headers: Resolved.ReadonlyHeaders;
    readonly bodyTruncated: boolean;
  }

  interface Response {
    readonly status: number;
    readonly statusText: string;
    readonly httpVersion: string;
    readonly url: string;
    readonly contentType: string | null;
    readonly headers: Resolved.ReadonlyHeaders;
    readonly durationMs: number;
    readonly sizeBytes: number;
    readonly bodyTruncated: boolean;
    text(): string;
    json(): any;
  }

  interface PreRequestGeneratorApi {
    readonly request: Request;
  }

  /** Generator-time response is nullable until an HTTP response exists. */
  interface PostResponseGeneratorApi extends PreRequestGeneratorApi {
    readonly response: Response | null;
  }

  interface TextRange {
    /** UTF-16 code-unit offset, matching JavaScript string indexing. */
    readonly start: number;
    /** Exclusive UTF-16 code-unit offset. */
    readonly end: number;
  }

  interface ExpressionOptions {
    /**
     * JavaScript root expression. Request body selections infer a root when
     * omitted. Response body selections do so only in post-response generators.
     */
    readonly root?: string;
  }

  interface Selection {
    readonly source: SelectionSource;
    readonly area: SelectionArea;
    readonly text: string;
    /** UTF-16 code-unit offset in the displayed source. */
    readonly start: number;
    /** Exclusive UTF-16 code-unit offset in the displayed source. */
    readonly end: number;
    readonly range: TextRange;
    readonly contentType: string | null;
    is(source: SelectionSource): boolean;
    /** Dot/bracket JSON path. Throws when no JSON node is mapped. */
    jsonPath(): string;
    /** RFC 6901 JSON Pointer. Throws when no JSON node is mapped. */
    jsonPointer(): string;
    /** JavaScript expression addressing the selected JSON node. */
    expression(options?: ExpressionOptions): string;
  }

  interface ResultOptions {
    /** UTF-16 code-unit offset for the preferred caret. */
    readonly cursor?: number | null;
  }

  interface Result {
    readonly text: string;
    readonly cursor: number | null;
  }

  interface Generator {
    readonly apiVersion: number;
    readonly category: Category;
    readonly outputLanguage: string;
    readonly selection: Selection | null;
    /** Appends text to this generator's bounded output buffer. */
    write(value: unknown): void;
    /** Builds an explicit generated result. A returned string is also valid. */
    result(text: unknown, options?: ResultOptions): Result;
  }
}

declare const snippet: ResolvedSnippet.Generator;

/** @deprecated Prefer `snippet.write(value)`. */
declare function write(value: unknown): void;
"#;

/// Lightweight discovery provider for executable snippet generators.
///
/// Plain snippets must use [`crate::script_intelligence::ScriptCompletionProvider`]
/// directly. This provider deliberately exposes only the generator runtime's
/// frozen snapshot API; environment mutation, request mutation, tests, and
/// assertions are not generator capabilities. Ordinary JavaScript semantics
/// still belong to the future TypeScript service.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SnippetIntelligenceProvider {
    context: SnippetEditorContext,
}

impl SnippetIntelligenceProvider {
    pub const fn new(context: SnippetEditorContext) -> Self {
        Self { context }
    }

    pub const fn context(&self) -> SnippetEditorContext {
        self.context
    }

    /// Synchronous entrypoint used by the GPUI provider and focused tests.
    pub fn completion_items_for_source(
        &self,
        source: &str,
        requested_offset: usize,
    ) -> Vec<CompletionItem> {
        generator_completion_items(self.context, source, requested_offset)
    }

    /// Returns documentation for a generator symbol under the pointer.
    pub fn hover_for_source(&self, source: &str, requested_offset: usize) -> Option<Hover> {
        generator_hover(self.context, source, requested_offset)
    }
}

impl CompletionProvider for SnippetIntelligenceProvider {
    fn supports_inline_completion(&self) -> bool {
        false
    }

    fn completions(
        &self,
        text: &Rope,
        offset: usize,
        _trigger: CompletionContext,
        _window: &mut Window,
        _cx: &mut Context<InputState>,
    ) -> Task<Result<CompletionResponse>> {
        Task::ready(Ok(CompletionResponse::Array(
            self.completion_items_for_source(&text.to_string(), offset),
        )))
    }

    fn is_completion_trigger(
        &self,
        _offset: usize,
        new_text: &str,
        _cx: &mut Context<InputState>,
    ) -> bool {
        generator_trigger(self.context, new_text)
    }

    fn is_completion_trigger_in_text(
        &self,
        text: &Rope,
        cursor_offset: usize,
        _edit_start: usize,
        new_text: &str,
        _cx: &mut Context<InputState>,
    ) -> bool {
        if generator_trigger(self.context, new_text) {
            let source = text.to_string();
            completion_context(&source, cursor_offset).is_some_and(|path_context| {
                SYMBOLS.iter().any(|symbol| {
                    symbol.is_available(self.context.phase())
                        && symbol.parent == path_context.parent.as_slice()
                        && symbol.label.starts_with(path_context.typed.as_str())
                })
            })
        } else {
            false
        }
    }
}

impl HoverProvider for SnippetIntelligenceProvider {
    fn hover(
        &self,
        text: &Rope,
        offset: usize,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Task<Result<Option<Hover>>> {
        Task::ready(Ok(self.hover_for_source(&text.to_string(), offset)))
    }
}

fn generator_completion_items(
    editor_context: SnippetEditorContext,
    source: &str,
    requested_offset: usize,
) -> Vec<CompletionItem> {
    if !editor_context.is_executable() {
        return Vec::new();
    }
    let Some(path_context) = completion_context(source, requested_offset) else {
        return Vec::new();
    };
    SYMBOLS
        .iter()
        .filter(|symbol| {
            symbol.is_available(editor_context.phase())
                && symbol.parent == path_context.parent.as_slice()
                && symbol.label.starts_with(path_context.typed.as_str())
        })
        .map(|symbol| completion_item(source, &path_context, *symbol))
        .collect()
}

fn generator_hover(
    editor_context: SnippetEditorContext,
    source: &str,
    requested_offset: usize,
) -> Option<Hover> {
    if !editor_context.is_executable() {
        return None;
    }
    let path_context = hover_context(source, requested_offset)?;
    let symbol = SYMBOLS.iter().find(|symbol| {
        symbol.is_available(editor_context.phase()) && symbol.full_path() == path_context.path
    })?;
    Some(symbol_hover(source, path_context.range, *symbol))
}

fn generator_trigger(context: SnippetEditorContext, new_text: &str) -> bool {
    context.is_executable()
        && !new_text.is_empty()
        && new_text
            .chars()
            .all(|character| is_identifier_continue(character) || character == '.')
}

#[derive(Clone, Copy)]
struct SymbolSpec {
    parent: &'static [&'static str],
    label: &'static str,
    kind: CompletionItemKind,
    detail: &'static str,
    documentation: &'static str,
    phase: Option<SnippetTargetPhase>,
}

impl SymbolSpec {
    const fn is_available(self, phase: SnippetTargetPhase) -> bool {
        matches!(
            (self.phase, phase),
            (None, _)
                | (
                    Some(SnippetTargetPhase::PreRequest),
                    SnippetTargetPhase::PreRequest
                )
                | (
                    Some(SnippetTargetPhase::PostResponse),
                    SnippetTargetPhase::PostResponse
                )
        )
    }

    fn full_path(self) -> Vec<&'static str> {
        self.parent
            .iter()
            .copied()
            .chain(std::iter::once(self.label))
            .collect()
    }

    fn signature(self) -> String {
        let path = self.full_path().join(".");
        if matches!(
            self.kind,
            CompletionItemKind::METHOD | CompletionItemKind::FUNCTION
        ) {
            format!("{path}{}", self.detail)
        } else {
            format!("{path}: {}", self.detail)
        }
    }
}

const ROOT: &[&str] = &[];
const API: &[&str] = &["api"];
const API_REQUEST: &[&str] = &["api", "request"];
const API_REQUEST_HEADERS: &[&str] = &["api", "request", "headers"];
const API_RESPONSE: &[&str] = &["api", "response"];
const API_RESPONSE_HEADERS: &[&str] = &["api", "response", "headers"];
const SNIPPET: &[&str] = &["snippet"];
const SELECTION: &[&str] = &["snippet", "selection"];
const SELECTION_RANGE: &[&str] = &["snippet", "selection", "range"];
const CONSOLE: &[&str] = &["console"];

const SYMBOLS: &[SymbolSpec] = &[
    pre_symbol(
        ROOT,
        "api",
        CompletionItemKind::VARIABLE,
        "ResolvedSnippet.PreRequestGeneratorApi",
        "Read-only generator snapshot for a pre-request snippet. It does not expose environment mutation or request mutation.",
    ),
    post_symbol(
        ROOT,
        "api",
        CompletionItemKind::VARIABLE,
        "ResolvedSnippet.PostResponseGeneratorApi",
        "Read-only generator snapshot for a post-response snippet. `api.response` is null until a response exists.",
    ),
    symbol(
        ROOT,
        "snippet",
        CompletionItemKind::VARIABLE,
        "ResolvedSnippet.Generator",
        "Read-only context and bounded output helpers available to executable snippet generators.",
    ),
    symbol(
        ROOT,
        "write",
        CompletionItemKind::FUNCTION,
        "(value: unknown): void",
        "Compatibility alias for `snippet.write(value)`. Prefer the namespaced method in new generators.",
    ),
    symbol(
        ROOT,
        "console",
        CompletionItemKind::VARIABLE,
        "Resolved.ScriptConsole",
        "Bounded generator console. Network and host runtime globals remain unavailable.",
    ),
    symbol(
        API,
        "request",
        CompletionItemKind::FIELD,
        "ResolvedSnippet.Request",
        "Frozen snapshot of the active request.",
    ),
    post_symbol(
        API,
        "response",
        CompletionItemKind::FIELD,
        "ResolvedSnippet.Response | null",
        "Frozen response snapshot, or null before an HTTP response exists. This is nullable at generator time even though inserted post-response scripts run after a response.",
    ),
    symbol(
        SNIPPET,
        "apiVersion",
        CompletionItemKind::FIELD,
        "number",
        "Version of the executable snippet generator API.",
    ),
    symbol(
        SNIPPET,
        "category",
        CompletionItemKind::FIELD,
        "ResolvedSnippet.Category",
        "Target script category: `pre-request` or `post-response`.",
    ),
    symbol(
        SNIPPET,
        "outputLanguage",
        CompletionItemKind::FIELD,
        "string",
        "Language identifier selected for the generated snippet text.",
    ),
    symbol(
        SNIPPET,
        "selection",
        CompletionItemKind::FIELD,
        "ResolvedSnippet.Selection | null",
        "Selected request or response text and structured-selection helpers, or null when nothing is selected.",
    ),
    symbol(
        SNIPPET,
        "write",
        CompletionItemKind::METHOD,
        "(value: unknown): void",
        "Appends text to this generator's bounded output buffer.",
    ),
    symbol(
        SNIPPET,
        "result",
        CompletionItemKind::METHOD,
        "(text: unknown, options?: ResultOptions): Result",
        "Builds an explicit generated result with an optional preferred caret offset.",
    ),
    symbol(
        API_REQUEST,
        "method",
        CompletionItemKind::FIELD,
        "Resolved.HttpMethod",
        "HTTP method in the request snapshot.",
    ),
    symbol(
        API_REQUEST,
        "url",
        CompletionItemKind::FIELD,
        "string",
        "URL in the request snapshot.",
    ),
    symbol(
        API_REQUEST,
        "headers",
        CompletionItemKind::FIELD,
        "Resolved.ReadonlyHeaders",
        "Read-only headers in the request snapshot.",
    ),
    symbol(
        API_REQUEST,
        "body",
        CompletionItemKind::FIELD,
        "string",
        "Request body snapshot as text.",
    ),
    symbol(
        API_REQUEST,
        "bodyMode",
        CompletionItemKind::FIELD,
        "Resolved.BodyMode",
        "Body mode of the request snapshot.",
    ),
    symbol(
        API_REQUEST,
        "rawBodyLanguage",
        CompletionItemKind::FIELD,
        "Resolved.RawBodyLanguage",
        "Raw body language of the request snapshot.",
    ),
    symbol(
        API_REQUEST,
        "bodyFields",
        CompletionItemKind::FIELD,
        "readonly Resolved.ReadonlyBodyField[]",
        "Read-only structured request body rows.",
    ),
    symbol(
        API_REQUEST,
        "bodyTruncated",
        CompletionItemKind::FIELD,
        "boolean",
        "Whether the generator-visible request body was bounded.",
    ),
    symbol(
        API_REQUEST_HEADERS,
        "has",
        CompletionItemKind::METHOD,
        "(name: unknown): boolean",
        "Checks for an enabled request header using a case-insensitive name.",
    ),
    symbol(
        API_REQUEST_HEADERS,
        "get",
        CompletionItemKind::METHOD,
        "(name: unknown): string | undefined",
        "Returns the first enabled request-header value with this case-insensitive name.",
    ),
    symbol(
        API_REQUEST_HEADERS,
        "getAll",
        CompletionItemKind::METHOD,
        "(name: unknown): string[]",
        "Returns every enabled request-header value with this case-insensitive name.",
    ),
    symbol(
        API_REQUEST_HEADERS,
        "toArray",
        CompletionItemKind::METHOD,
        "(): { enabled: boolean; name: string; value: string }[]",
        "Returns detached copies of the request header rows.",
    ),
    post_symbol(
        API_RESPONSE,
        "status",
        CompletionItemKind::FIELD,
        "number",
        "HTTP response status code.",
    ),
    post_symbol(
        API_RESPONSE,
        "statusText",
        CompletionItemKind::FIELD,
        "string",
        "HTTP response reason phrase.",
    ),
    post_symbol(
        API_RESPONSE,
        "httpVersion",
        CompletionItemKind::FIELD,
        "string",
        "Negotiated HTTP version.",
    ),
    post_symbol(
        API_RESPONSE,
        "url",
        CompletionItemKind::FIELD,
        "string",
        "Final response URL.",
    ),
    post_symbol(
        API_RESPONSE,
        "contentType",
        CompletionItemKind::FIELD,
        "string | null",
        "Response Content-Type, or null when absent.",
    ),
    post_symbol(
        API_RESPONSE,
        "headers",
        CompletionItemKind::FIELD,
        "Resolved.ReadonlyHeaders",
        "Read-only response headers.",
    ),
    post_symbol(
        API_RESPONSE,
        "durationMs",
        CompletionItemKind::FIELD,
        "number",
        "Request duration in whole milliseconds.",
    ),
    post_symbol(
        API_RESPONSE,
        "sizeBytes",
        CompletionItemKind::FIELD,
        "number",
        "Full received response-body size.",
    ),
    post_symbol(
        API_RESPONSE,
        "bodyTruncated",
        CompletionItemKind::FIELD,
        "boolean",
        "Whether the generator-visible response body was bounded.",
    ),
    post_symbol(
        API_RESPONSE,
        "text",
        CompletionItemKind::METHOD,
        "(): string",
        "Returns the generator-visible response body as text.",
    ),
    post_symbol(
        API_RESPONSE,
        "json",
        CompletionItemKind::METHOD,
        "(): any",
        "Parses the response body as JSON and throws when invalid.",
    ),
    post_symbol(
        API_RESPONSE_HEADERS,
        "has",
        CompletionItemKind::METHOD,
        "(name: unknown): boolean",
        "Checks for a response header using a case-insensitive name.",
    ),
    post_symbol(
        API_RESPONSE_HEADERS,
        "get",
        CompletionItemKind::METHOD,
        "(name: unknown): string | undefined",
        "Returns the first response-header value with this case-insensitive name.",
    ),
    post_symbol(
        API_RESPONSE_HEADERS,
        "getAll",
        CompletionItemKind::METHOD,
        "(name: unknown): string[]",
        "Returns every response-header value with this case-insensitive name.",
    ),
    post_symbol(
        API_RESPONSE_HEADERS,
        "toArray",
        CompletionItemKind::METHOD,
        "(): { enabled: boolean; name: string; value: string }[]",
        "Returns detached copies of the response header rows.",
    ),
    symbol(
        SELECTION,
        "area",
        CompletionItemKind::FIELD,
        "ResolvedSnippet.SelectionArea",
        "Editor area containing the selection: script, URL, headers, or body.",
    ),
    symbol(
        SELECTION,
        "source",
        CompletionItemKind::FIELD,
        "ResolvedSnippet.SelectionSource",
        "Whether the selected text came from the request or response.",
    ),
    symbol(
        SELECTION,
        "text",
        CompletionItemKind::FIELD,
        "string",
        "Exact selected text from the displayed source.",
    ),
    symbol(
        SELECTION,
        "start",
        CompletionItemKind::FIELD,
        "number",
        "UTF-16 code-unit offset at the start of the displayed selection.",
    ),
    symbol(
        SELECTION,
        "end",
        CompletionItemKind::FIELD,
        "number",
        "Exclusive UTF-16 code-unit offset at the end of the displayed selection.",
    ),
    symbol(
        SELECTION,
        "range",
        CompletionItemKind::FIELD,
        "ResolvedSnippet.TextRange",
        "Frozen start/end range in UTF-16 code units.",
    ),
    symbol(
        SELECTION,
        "contentType",
        CompletionItemKind::FIELD,
        "string | null",
        "Content type associated with the selected editor area, or null when unavailable.",
    ),
    symbol(
        SELECTION,
        "is",
        CompletionItemKind::METHOD,
        "(source: SelectionSource): boolean",
        "Checks whether the selection came from `request` or `response`.",
    ),
    symbol(
        SELECTION,
        "jsonPath",
        CompletionItemKind::METHOD,
        "(): string",
        "Returns a safe dot/bracket JSON path. Throws when the selection is not mapped to a JSON node.",
    ),
    symbol(
        SELECTION,
        "jsonPointer",
        CompletionItemKind::METHOD,
        "(): string",
        "Returns an RFC 6901 JSON Pointer. Throws when the selection is not mapped to a JSON node.",
    ),
    symbol(
        SELECTION,
        "expression",
        CompletionItemKind::METHOD,
        "(options?: { root?: string }): string",
        "Builds a JavaScript expression for the selected JSON node. Request bodies infer a root; response bodies do so only for post-response generators.",
    ),
    symbol(
        SELECTION_RANGE,
        "start",
        CompletionItemKind::FIELD,
        "number",
        "UTF-16 start offset.",
    ),
    symbol(
        SELECTION_RANGE,
        "end",
        CompletionItemKind::FIELD,
        "number",
        "Exclusive UTF-16 end offset.",
    ),
    symbol(
        CONSOLE,
        "log",
        CompletionItemKind::METHOD,
        "(...values: unknown[]): void",
        "Writes a bounded log row.",
    ),
    symbol(
        CONSOLE,
        "info",
        CompletionItemKind::METHOD,
        "(...values: unknown[]): void",
        "Writes a bounded info row.",
    ),
    symbol(
        CONSOLE,
        "warn",
        CompletionItemKind::METHOD,
        "(...values: unknown[]): void",
        "Writes a bounded warning row.",
    ),
    symbol(
        CONSOLE,
        "error",
        CompletionItemKind::METHOD,
        "(...values: unknown[]): void",
        "Writes a bounded error row.",
    ),
    symbol(
        CONSOLE,
        "debug",
        CompletionItemKind::METHOD,
        "(...values: unknown[]): void",
        "Writes a bounded debug row.",
    ),
];

const fn symbol(
    parent: &'static [&'static str],
    label: &'static str,
    kind: CompletionItemKind,
    detail: &'static str,
    documentation: &'static str,
) -> SymbolSpec {
    SymbolSpec {
        parent,
        label,
        kind,
        detail,
        documentation,
        phase: None,
    }
}

const fn pre_symbol(
    parent: &'static [&'static str],
    label: &'static str,
    kind: CompletionItemKind,
    detail: &'static str,
    documentation: &'static str,
) -> SymbolSpec {
    SymbolSpec {
        parent,
        label,
        kind,
        detail,
        documentation,
        phase: Some(SnippetTargetPhase::PreRequest),
    }
}

const fn post_symbol(
    parent: &'static [&'static str],
    label: &'static str,
    kind: CompletionItemKind,
    detail: &'static str,
    documentation: &'static str,
) -> SymbolSpec {
    SymbolSpec {
        parent,
        label,
        kind,
        detail,
        documentation,
        phase: Some(SnippetTargetPhase::PostResponse),
    }
}

struct CompletionPath {
    parent: Vec<String>,
    typed: String,
    replace_start: usize,
    offset: usize,
}

fn completion_context(source: &str, requested_offset: usize) -> Option<CompletionPath> {
    let offset = clipped_char_boundary(source, requested_offset);
    if !is_code_position(&source[..offset]) {
        return None;
    }

    let mut start = offset;
    while start > 0 {
        let character = source[..start].chars().next_back()?;
        if is_identifier_continue(character) || matches!(character, '.' | '?') {
            start -= character.len_utf8();
        } else {
            break;
        }
    }
    let fragment = &source[start..offset];
    if fragment.is_empty() {
        return None;
    }
    let normalized = fragment.replace("?.", ".");
    // A preceding compact ternary/nullish expression is not part of the
    // member path. Optional-chain question marks were removed above.
    let normalized = normalized.rsplit('?').next().unwrap_or_default();
    let mut segments = normalized.split('.').collect::<Vec<_>>();
    let typed = segments.pop().unwrap_or_default();
    if segments.iter().any(|segment| !is_identifier(segment))
        || (!typed.is_empty() && !is_identifier_fragment(typed))
    {
        return None;
    }

    let replace_start = offset.saturating_sub(typed.len());
    Some(CompletionPath {
        parent: segments.into_iter().map(str::to_owned).collect(),
        typed: typed.to_owned(),
        replace_start,
        offset,
    })
}

struct HoverPath {
    path: Vec<&'static str>,
    range: std::ops::Range<usize>,
}

fn hover_context(source: &str, requested_offset: usize) -> Option<HoverPath> {
    if source.is_empty() {
        return None;
    }
    let offset = clipped_char_boundary(source, requested_offset);
    let mut probe = offset.min(source.len());
    if probe == source.len()
        || !source[probe..]
            .chars()
            .next()
            .is_some_and(is_identifier_continue)
    {
        let previous = source[..probe].chars().next_back()?;
        if !is_identifier_continue(previous) {
            return None;
        }
        probe -= previous.len_utf8();
    }

    let mut start = probe;
    while start > 0 {
        let previous = source[..start].chars().next_back()?;
        if !is_identifier_continue(previous) {
            break;
        }
        start -= previous.len_utf8();
    }
    let mut end = probe;
    while end < source.len() {
        let character = source[end..].chars().next()?;
        if !is_identifier_continue(character) {
            break;
        }
        end += character.len_utf8();
    }
    if start == end || !is_code_position(&source[..start]) {
        return None;
    }

    let mut path_start = start;
    while path_start > 0 {
        let dot_start = path_start.checked_sub(1)?;
        if source.as_bytes().get(dot_start) != Some(&b'.') {
            break;
        }
        let separator_start =
            if dot_start > 0 && source.as_bytes().get(dot_start - 1) == Some(&b'?') {
                dot_start - 1
            } else {
                dot_start
            };
        let mut identifier_start = separator_start;
        while identifier_start > 0 {
            let previous = source[..identifier_start].chars().next_back()?;
            if !is_identifier_continue(previous) {
                break;
            }
            identifier_start -= previous.len_utf8();
        }
        if identifier_start == separator_start {
            break;
        }
        path_start = identifier_start;
    }

    let candidate_path = source[path_start..end].replace("?.", ".");
    let candidate = candidate_path.split('.').collect::<Vec<_>>();
    let symbol = SYMBOLS
        .iter()
        .find(|symbol| symbol.full_path().as_slice() == candidate.as_slice())?;
    Some(HoverPath {
        path: symbol.full_path(),
        range: start..end,
    })
}

fn completion_item(source: &str, context: &CompletionPath, symbol: SymbolSpec) -> CompletionItem {
    CompletionItem {
        label: symbol.label.to_owned(),
        kind: Some(symbol.kind),
        detail: Some(symbol.detail.to_owned()),
        documentation: Some(Documentation::String(symbol.documentation.to_owned())),
        text_edit: Some(CompletionTextEdit::Edit(TextEdit {
            range: source_range(source, context.replace_start, context.offset),
            new_text: symbol.label.to_owned(),
        })),
        ..Default::default()
    }
}

fn symbol_hover(source: &str, range: std::ops::Range<usize>, symbol: SymbolSpec) -> Hover {
    Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: format!(
                "```javascript\n{}\n```\n\n{}\n\n_Available in executable snippet generators._",
                symbol.signature(),
                symbol.documentation,
            ),
        }),
        range: Some(source_range(source, range.start, range.end)),
    }
}

fn is_identifier(value: &str) -> bool {
    let mut characters = value.chars();
    characters.next().is_some_and(is_identifier_start) && characters.all(is_identifier_continue)
}

fn is_identifier_fragment(value: &str) -> bool {
    value.chars().next().is_some_and(is_identifier_start)
        && value.chars().skip(1).all(is_identifier_continue)
}

fn is_identifier_start(character: char) -> bool {
    character == '_' || character == '$' || character.is_alphabetic()
}

fn is_identifier_continue(character: char) -> bool {
    is_identifier_start(character) || character.is_ascii_digit()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LexicalState {
    Code,
    SingleQuoted,
    DoubleQuoted,
    Template,
    LineComment,
    BlockComment,
}

fn is_code_position(prefix: &str) -> bool {
    let mut state = LexicalState::Code;
    let mut escaped = false;
    let mut template_expression_depths = Vec::<usize>::new();
    let mut characters = prefix.chars().peekable();

    while let Some(character) = characters.next() {
        match state {
            LexicalState::Code => match character {
                '\'' => {
                    state = LexicalState::SingleQuoted;
                    escaped = false;
                }
                '"' => {
                    state = LexicalState::DoubleQuoted;
                    escaped = false;
                }
                '`' => {
                    state = LexicalState::Template;
                    escaped = false;
                }
                '/' if characters.peek() == Some(&'/') => {
                    characters.next();
                    state = LexicalState::LineComment;
                }
                '/' if characters.peek() == Some(&'*') => {
                    characters.next();
                    state = LexicalState::BlockComment;
                }
                '{' => {
                    if let Some(depth) = template_expression_depths.last_mut() {
                        *depth = depth.saturating_add(1);
                    }
                }
                '}' if template_expression_depths.last() == Some(&0) => {
                    template_expression_depths.pop();
                    state = LexicalState::Template;
                }
                '}' => {
                    if let Some(depth) = template_expression_depths.last_mut() {
                        *depth = depth.saturating_sub(1);
                    }
                }
                _ => {}
            },
            LexicalState::SingleQuoted => {
                if escaped {
                    escaped = false;
                } else if character == '\\' {
                    escaped = true;
                } else if character == '\'' {
                    state = LexicalState::Code;
                }
            }
            LexicalState::DoubleQuoted => {
                if escaped {
                    escaped = false;
                } else if character == '\\' {
                    escaped = true;
                } else if character == '"' {
                    state = LexicalState::Code;
                }
            }
            LexicalState::Template => {
                if escaped {
                    escaped = false;
                } else if character == '\\' {
                    escaped = true;
                } else if character == '`' {
                    state = LexicalState::Code;
                } else if character == '$' && characters.peek() == Some(&'{') {
                    characters.next();
                    template_expression_depths.push(0);
                    state = LexicalState::Code;
                }
            }
            LexicalState::LineComment => {
                if character == '\n' {
                    state = LexicalState::Code;
                }
            }
            LexicalState::BlockComment => {
                if character == '*' && characters.peek() == Some(&'/') {
                    characters.next();
                    state = LexicalState::Code;
                }
            }
        }
    }

    state == LexicalState::Code
}

fn clipped_char_boundary(source: &str, requested_offset: usize) -> usize {
    let mut offset = requested_offset.min(source.len());
    while !source.is_char_boundary(offset) {
        offset = offset.saturating_sub(1);
    }
    offset
}

fn source_range(source: &str, start: usize, end: usize) -> Range {
    Range::new(source_position(source, start), source_position(source, end))
}

fn source_position(source: &str, requested_offset: usize) -> Position {
    let offset = clipped_char_boundary(source, requested_offset);
    let prefix = &source[..offset];
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count();
    let line_start = prefix.rfind('\n').map_or(0, |index| index + 1);
    Position::new(
        u32::try_from(line).unwrap_or(u32::MAX),
        u32::try_from(source[line_start..offset].encode_utf16().count()).unwrap_or(u32::MAX),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(items: Vec<CompletionItem>) -> Vec<String> {
        items.into_iter().map(|item| item.label).collect()
    }

    fn provider(context: SnippetEditorContext) -> SnippetIntelligenceProvider {
        SnippetIntelligenceProvider::new(context)
    }

    fn executable() -> SnippetIntelligenceProvider {
        provider(SnippetEditorContext::ExecutableGenerator(
            SnippetTargetPhase::PostResponse,
        ))
    }

    #[test]
    fn executable_virtual_document_accepts_function_body_returns_and_maps_utf16_offsets() {
        let context = SnippetEditorContext::ExecutableGenerator(SnippetTargetPhase::PostResponse);
        let editor_source = "const marker = '🎉';\nreturn marker;";
        let document = context.virtual_document(editor_source);
        assert!(document.source().contains("function ()"));
        assert!(document.source().contains(editor_source));
        assert_eq!(
            document.virtual_to_editor_utf16(document.editor_start_utf16()),
            Some(0)
        );
        let editor_end = editor_source.encode_utf16().count();
        let virtual_end = document.editor_to_virtual_utf16(editor_end).unwrap();
        assert_eq!(
            document.virtual_to_editor_utf16(virtual_end),
            Some(editor_end)
        );
        assert_eq!(
            document.virtual_to_editor_utf16(document.editor_start_utf16() - 1),
            None
        );

        let plain = SnippetEditorContext::PlainJavaScript(SnippetTargetPhase::PreRequest)
            .virtual_document(editor_source);
        assert_eq!(plain.source(), editor_source);
        assert_eq!(plain.editor_start_utf16(), 0);
    }

    #[test]
    fn contexts_keep_generator_globals_out_of_plain_snippets() {
        let plain_context = SnippetEditorContext::PlainJavaScript(SnippetTargetPhase::PreRequest);
        let plain = provider(plain_context);
        assert!(plain.completion_items_for_source("sni", 3).is_empty());
        assert!(plain.hover_for_source("snippet", 3).is_none());
        assert!(plain.completion_items_for_source("ap", 2).is_empty());
        assert!(
            plain_context
                .ambient_declarations()
                .phase()
                .contains("Resolved.PreRequestApi")
        );
        assert!(plain_context.ambient_declarations().generator().is_none());

        let executable_labels = labels(executable().completion_items_for_source("sni", 3));
        assert_eq!(executable_labels, vec!["snippet"]);
        assert!(
            SnippetEditorContext::ExecutableGenerator(SnippetTargetPhase::PostResponse)
                .ambient_declarations()
                .generator()
                .is_some_and(|declarations| {
                    declarations.contains("declare const snippet: ResolvedSnippet.Generator")
                })
        );
    }

    #[test]
    fn ambient_api_is_phase_accurate_and_generator_post_response_is_nullable() {
        let plain_post = SnippetEditorContext::PlainJavaScript(SnippetTargetPhase::PostResponse)
            .ambient_declarations();
        assert!(plain_post.phase().contains("Resolved.PostResponseApi"));
        assert!(!plain_post.phase().contains("PostResponseGeneratorApi"));
        assert!(plain_post.generator().is_none());

        let executable_post =
            SnippetEditorContext::ExecutableGenerator(SnippetTargetPhase::PostResponse)
                .ambient_declarations();
        assert!(
            executable_post
                .phase()
                .contains("ResolvedSnippet.PostResponseGeneratorApi")
        );
        assert!(executable_post.generator().is_some());
        assert!(SNIPPET_GENERATOR_DECLARATIONS.contains("readonly response: Response | null"));
        assert!(SNIPPET_GENERATOR_DECLARATIONS.contains("interface PostResponseGeneratorApi"));
    }

    #[test]
    fn executable_api_is_read_only_and_phase_accurate() {
        let pre = provider(SnippetEditorContext::ExecutableGenerator(
            SnippetTargetPhase::PreRequest,
        ));
        let post = executable();
        assert_eq!(
            labels(pre.completion_items_for_source("api.", 4)),
            vec!["request"]
        );
        assert_eq!(
            labels(post.completion_items_for_source("api.", 4)),
            vec!["request", "response"]
        );
        assert!(pre.completion_items_for_source("api.res", 7).is_empty());
        assert!(
            post.completion_items_for_source("api.environment", 15)
                .is_empty()
        );
        assert_eq!(
            labels(post.completion_items_for_source("api.request.bodyT", 17)),
            vec!["bodyTruncated"]
        );
    }

    #[test]
    fn executable_namespace_completes_response_and_selection_helpers() {
        assert_eq!(
            labels(executable().completion_items_for_source("snippet.res", 11)),
            vec!["result"]
        );
        assert_eq!(
            labels(executable().completion_items_for_source("api.response.st", 15)),
            vec!["status", "statusText"]
        );
        assert_eq!(
            labels(executable().completion_items_for_source("snippet.selection.json", 22)),
            vec!["jsonPath", "jsonPointer"]
        );
        assert_eq!(
            labels(executable().completion_items_for_source("snippet.selection.ex", 20)),
            vec!["expression"]
        );
    }

    #[test]
    fn optional_chains_complete_nullable_response_and_selection_members() {
        let provider = executable();
        assert_eq!(
            labels(provider.completion_items_for_source("api.response?.st", 16)),
            vec!["status", "statusText"]
        );
        assert_eq!(
            labels(provider.completion_items_for_source("snippet.selection?.text", 23)),
            vec!["text"]
        );
        assert_eq!(
            labels(provider.completion_items_for_source("api?.request?.bodyT", 19)),
            vec!["bodyTruncated"]
        );
    }

    #[test]
    fn header_bags_complete_the_runtime_methods_with_phase_filtering() {
        let pre = provider(SnippetEditorContext::ExecutableGenerator(
            SnippetTargetPhase::PreRequest,
        ));
        let post = executable();
        let methods = vec!["has", "get", "getAll", "toArray"];

        assert_eq!(
            labels(pre.completion_items_for_source("api.request.headers.", 20)),
            methods
        );
        assert_eq!(
            labels(post.completion_items_for_source("api.response?.headers.", 22)),
            methods
        );
        assert_eq!(
            labels(post.completion_items_for_source("api.response?.headers.get", 25)),
            vec!["get", "getAll"]
        );
        assert!(
            pre.completion_items_for_source("api.response?.headers.", 22)
                .is_empty()
        );
    }

    #[test]
    fn template_interpolations_are_code_but_template_text_is_not() {
        let provider = executable();
        let interpolation = "const value = `status: ${api.response?.st";
        assert_eq!(
            labels(provider.completion_items_for_source(interpolation, interpolation.len())),
            vec!["status", "statusText"]
        );

        for literal_text in [
            "const value = `api.response.st",
            "const value = `status: ${api.response?.status} api.response.st",
            "const value = `escaped: \\${api.response.st",
        ] {
            assert!(
                provider
                    .completion_items_for_source(literal_text, literal_text.len())
                    .is_empty(),
                "unexpected completion in template text: {literal_text:?}"
            );
        }

        let nested_expression = "const value = `status: ${{ response: api.response?.st";
        assert_eq!(
            labels(
                provider.completion_items_for_source(nested_expression, nested_expression.len())
            ),
            vec!["status", "statusText"]
        );
    }

    #[test]
    fn ambient_types_cover_the_generator_contract_and_write_compatibility_alias() {
        let declarations = SNIPPET_GENERATOR_DECLARATIONS;
        for expected in [
            "interface PreRequestGeneratorApi",
            "interface PostResponseGeneratorApi",
            "readonly request: Request",
            "readonly response: Response | null",
            "readonly apiVersion: number",
            "readonly category: Category",
            "readonly outputLanguage: string",
            "readonly status: number",
            "readonly statusText: string",
            "readonly contentType: string | null",
            "readonly area: SelectionArea",
            "readonly range: TextRange",
            "readonly start: number",
            "readonly end: number",
            "text(): string",
            "json(): any",
            "is(source: SelectionSource): boolean",
            "jsonPath(): string",
            "jsonPointer(): string",
            "expression(options?: ExpressionOptions): string",
            "write(value: unknown): void",
            "result(text: unknown, options?: ResultOptions): Result",
            "declare function write(value: unknown): void",
        ] {
            assert!(
                declarations.contains(expected),
                "missing declaration: {expected}"
            );
        }
        assert!(!declarations.contains("readonly selections:"));
        assert!(!declarations.contains("readonly selections?:"));
        assert!(!declarations.contains("readonly request: Request;\n    /** Current response"));
    }

    #[test]
    fn lightweight_provider_does_not_complete_inside_strings_or_comments() {
        let provider = executable();
        for source in [
            "const value = \"snippet.sel",
            "const value = 'snippet.sel",
            "const value = `snippet.sel",
            "// snippet.sel",
            "/* snippet.sel",
        ] {
            assert!(
                provider
                    .completion_items_for_source(source, source.len())
                    .is_empty(),
                "unexpected completion for {source:?}"
            );
        }
        let after_comment = "// snippet.sel\nsnippet.sel";
        assert_eq!(
            labels(provider.completion_items_for_source(after_comment, after_comment.len())),
            vec!["selection"]
        );
    }

    #[test]
    fn completion_edits_use_utf16_positions_after_unicode() {
        let source = "const marker = \"🎉\"; snippet.sel";
        let item = executable()
            .completion_items_for_source(source, source.len())
            .into_iter()
            .find(|item| item.label == "selection")
            .expect("selection completion");
        let CompletionTextEdit::Edit(edit) = item.text_edit.expect("completion edit") else {
            panic!("expected simple completion edit");
        };
        assert_eq!(
            edit.range.start.character,
            "const marker = \"🎉\"; snippet.".encode_utf16().count() as u32
        );
        assert_eq!(
            edit.range.end.character,
            source.encode_utf16().count() as u32
        );

        let template_source = "const label = `🎉 ${api.response?.st";
        let template_item = executable()
            .completion_items_for_source(template_source, template_source.len())
            .into_iter()
            .find(|item| item.label == "status")
            .expect("status completion in template interpolation");
        let CompletionTextEdit::Edit(template_edit) =
            template_item.text_edit.expect("template completion edit")
        else {
            panic!("expected simple template completion edit");
        };
        assert_eq!(
            template_edit.range,
            Range::new(
                Position::new(
                    0,
                    "const label = `🎉 ${api.response?.".encode_utf16().count() as u32,
                ),
                Position::new(0, template_source.encode_utf16().count() as u32),
            )
        );
    }

    #[test]
    fn hover_resolves_full_paths_and_scopes_the_identifier_range() {
        let source = "snippet.selection.jsonPointer()";
        let hover = executable()
            .hover_for_source(source, source.find("jsonPointer").unwrap() + 3)
            .expect("selection helper hover");
        let HoverContents::Markup(markup) = hover.contents else {
            panic!("expected markdown hover");
        };
        assert!(
            markup
                .value
                .contains("snippet.selection.jsonPointer(): string")
        );
        assert_eq!(
            hover.range,
            Some(Range::new(Position::new(0, 18), Position::new(0, 29)))
        );

        let alias = executable()
            .hover_for_source("write('value')", 2)
            .expect("write alias hover");
        let HoverContents::Markup(alias_markup) = alias.contents else {
            panic!("expected markdown hover");
        };
        assert!(alias_markup.value.contains("Compatibility alias"));

        let optional_source = "api.response?.status";
        let optional_hover = executable()
            .hover_for_source(optional_source, optional_source.len())
            .expect("optional response hover");
        let HoverContents::Markup(optional_markup) = optional_hover.contents else {
            panic!("expected markdown hover");
        };
        assert!(
            optional_markup
                .value
                .contains("api.response.status: number")
        );
        assert_eq!(
            optional_hover.range,
            Some(Range::new(Position::new(0, 14), Position::new(0, 20)))
        );

        let optional_selection = "snippet.selection?.text";
        assert!(
            executable()
                .hover_for_source(optional_selection, optional_selection.len())
                .is_some()
        );

        let pre = provider(SnippetEditorContext::ExecutableGenerator(
            SnippetTargetPhase::PreRequest,
        ));
        assert!(
            pre.hover_for_source(optional_source, optional_source.len())
                .is_none()
        );

        let request_url = pre
            .hover_for_source("api.request.url", "api.request.url".len())
            .expect("request URL hover");
        let HoverContents::Markup(request_url_markup) = request_url.contents else {
            panic!("expected markdown hover");
        };
        assert!(request_url_markup.value.contains("request snapshot"));
        assert!(!request_url_markup.value.contains("Resolved URL"));
    }
}

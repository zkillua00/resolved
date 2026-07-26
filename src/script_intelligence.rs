//! Lightweight editor intelligence for the API Tester script runtime.
//!
//! This module intentionally models variable *names* only. Environment and
//! collection values (including secrets) must never enter completion items,
//! diagnostics, logs, or the provider's retained state.

use std::{cell::RefCell, collections::BTreeSet, rc::Rc};

use anyhow::Result;
use gpui::{Context, Task, Window};
use gpui_component::input::{CompletionProvider, InputState, Rope};
use lsp_types::{
    CompletionContext, CompletionItem, CompletionItemKind, CompletionResponse, CompletionTextEdit,
    Diagnostic, DiagnosticSeverity, Documentation, NumberOrString, Position, Range, TextEdit,
};

const DIAGNOSTIC_SOURCE: &str = "api-tester";
const MISSING_VARIABLE_CODE: &str = "missing-script-variable";
const DISABLED_VARIABLE_CODE: &str = "disabled-script-variable";

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

/// A value-blind snapshot of the variable names visible to a script.
///
/// The fields are private and the constructor accepts names, not key/value
/// pairs, so secret values have no path into the editor-intelligence layer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScriptVariableCatalog {
    environment_names: BTreeSet<String>,
    disabled_environment_names: BTreeSet<String>,
    collection_names: BTreeSet<String>,
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
            disabled_environment_names,
            collection_names: normalized_names(collection_names),
        }
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

    /// Replaces the value-blind snapshot in place. Providers sharing this
    /// catalog see the selected-environment change on their next invocation.
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
#[derive(Clone, Debug)]
pub struct ScriptCompletionProvider {
    phase: ScriptEditorPhase,
    variables: ScriptVariableCatalogHandle,
}

impl ScriptCompletionProvider {
    pub fn new(phase: ScriptEditorPhase, variables: ScriptVariableCatalogHandle) -> Self {
        Self { phase, variables }
    }

    /// Synchronous completion entrypoint used by the GPUI provider and focused
    /// unit tests.
    pub fn completion_items_for_source(&self, source: &str, offset: usize) -> Vec<CompletionItem> {
        completion_items(source, offset, self.phase, &self.variables.borrow())
    }
}

impl CompletionProvider for ScriptCompletionProvider {
    fn completions(
        &self,
        text: &Rope,
        offset: usize,
        _trigger: CompletionContext,
        _window: &mut Window,
        _cx: &mut Context<InputState>,
    ) -> Task<Result<CompletionResponse>> {
        let source = text.to_string();
        let items = self.completion_items_for_source(&source, offset);
        Task::ready(Ok(CompletionResponse::Array(items)))
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
}

/// Emits conservative warnings for missing literal variable reads.
///
/// Only the exact calls `api.environment.get("literal")` and
/// `api.variables.get("literal")` are checked. Comments, string contents,
/// dynamic arguments, `has`, and write calls are deliberately ignored. A
/// preceding literal `api.environment.set("name", value)` makes that name
/// available to later reads in the same source.
pub fn diagnostics_for_source(source: &str, variables: &ScriptVariableCatalog) -> Vec<Diagnostic> {
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

    diagnostics
}

#[derive(Clone, Copy)]
struct CompletionSpec {
    label: &'static str,
    kind: CompletionItemKind,
    detail: &'static str,
    documentation: &'static str,
}

const ROOT_MEMBERS_PRE: &[CompletionSpec] = &[
    field("request", "Request", "The request being prepared."),
    field(
        "environment",
        "EnvironmentVariables",
        "Variables from the selected environment.",
    ),
    field(
        "variables",
        "Variables",
        "Environment variables with collection-variable fallback.",
    ),
    field("console", "Console", "Bounded script console."),
];

const ROOT_MEMBERS_POST: &[CompletionSpec] = &[
    field("request", "Readonly<Request>", "The request that was sent."),
    field("response", "Response", "The received response."),
    field(
        "environment",
        "EnvironmentVariables",
        "Variables from the selected environment.",
    ),
    field(
        "variables",
        "Variables",
        "Environment variables with collection-variable fallback.",
    ),
    field("console", "Console", "Bounded script console."),
    method(
        "test",
        "(name, callback) -> void",
        "Records a post-response test result.",
    ),
    method(
        "assert",
        "(condition, message?) -> void",
        "Throws when a test assertion is false.",
    ),
];

const REQUEST_MEMBERS: &[CompletionSpec] = &[
    field("method", "string", "HTTP method."),
    field("url", "string", "Request URL."),
    field("body", "string", "Raw request body."),
    field("bodyMode", "string", "Request body mode."),
    field(
        "rawBodyLanguage",
        "string",
        "Syntax language selected for a raw body.",
    ),
    field(
        "bodyFields",
        "BodyField[]",
        "Form or multipart body fields.",
    ),
    field("headers", "Headers", "Request header bag."),
];

const RESPONSE_MEMBERS: &[CompletionSpec] = &[
    field("status", "number", "HTTP response status."),
    field("statusText", "string", "HTTP response status text."),
    field("httpVersion", "string", "Negotiated HTTP version."),
    field("url", "string", "Final response URL."),
    field("headers", "ReadonlyHeaders", "Response header bag."),
    field("durationMs", "number", "Request duration in milliseconds."),
    field("sizeBytes", "number", "Received response size."),
    field(
        "truncated",
        "boolean",
        "Whether the response body was truncated.",
    ),
    field(
        "bodyBase64",
        "string | null",
        "Base64 response body when it is not UTF-8 text.",
    ),
    method("text", "() -> string", "Returns the response body as text."),
    method("json", "() -> unknown", "Parses the response body as JSON."),
];

const READONLY_HEADER_MEMBERS: &[CompletionSpec] = &[
    method("has", "(name) -> boolean", "Checks for an enabled header."),
    method(
        "get",
        "(name) -> string | undefined",
        "Returns the first enabled header value.",
    ),
    method(
        "getAll",
        "(name) -> string[]",
        "Returns all enabled values for a header.",
    ),
    method(
        "toArray",
        "() -> Header[]",
        "Returns a detached array of headers.",
    ),
];

const MUTABLE_HEADER_MEMBERS: &[CompletionSpec] = &[
    method("has", "(name) -> boolean", "Checks for an enabled header."),
    method(
        "get",
        "(name) -> string | undefined",
        "Returns the first enabled header value.",
    ),
    method(
        "getAll",
        "(name) -> string[]",
        "Returns all enabled values for a header.",
    ),
    method(
        "set",
        "(name, value) -> void",
        "Sets one request header value.",
    ),
    method(
        "append",
        "(name, value) -> void",
        "Appends a request header.",
    ),
    method(
        "remove",
        "(name) -> void",
        "Removes request headers with this name.",
    ),
    method(
        "toArray",
        "() -> Header[]",
        "Returns a detached array of headers.",
    ),
];

const ENVIRONMENT_MEMBERS: &[CompletionSpec] = &[
    method("has", "(key) -> boolean", "Checks the active environment."),
    method(
        "get",
        "(key) -> string | undefined",
        "Reads an active-environment variable.",
    ),
    method(
        "set",
        "(key, value) -> void",
        "Sets an active-environment variable after this script succeeds.",
    ),
    method(
        "unset",
        "(key) -> void",
        "Removes an active-environment variable after this script succeeds.",
    ),
    method(
        "toObject",
        "() -> Record<string, string>",
        "Returns a detached object of active-environment variables.",
    ),
];

const VARIABLE_MEMBERS: &[CompletionSpec] = &[
    method(
        "has",
        "(key) -> boolean",
        "Checks the environment, then collection variables.",
    ),
    method(
        "get",
        "(key) -> string | undefined",
        "Reads the environment, then collection variables.",
    ),
    method(
        "toObject",
        "() -> Record<string, string>",
        "Returns merged collection and environment variables.",
    ),
];

const CONSOLE_MEMBERS: &[CompletionSpec] = &[
    method("log", "(...values) -> void", "Writes a bounded log entry."),
    method("info", "(...values) -> void", "Writes an info log entry."),
    method("warn", "(...values) -> void", "Writes a warning log entry."),
    method("error", "(...values) -> void", "Writes an error log entry."),
    method("debug", "(...values) -> void", "Writes a debug log entry."),
];

const GLOBALS: &[CompletionSpec] = &[
    field("api", "ApiTesterRuntime", "Sandboxed API Tester runtime."),
    field("console", "Console", "Bounded script console."),
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
) -> Vec<CompletionItem> {
    let offset = clipped_char_boundary(source, requested_offset);
    let prefix = &source[..offset];
    let lexed = lex(prefix);

    if lexed.ended_in_comment_or_template {
        return Vec::new();
    }

    if let Some(string_context) = variable_string_completion_context(&lexed.tokens, offset) {
        let names: Box<dyn Iterator<Item = &str> + '_> = match string_context.namespace {
            VariableNamespace::Environment => Box::new(variables.environment_names()),
            VariableNamespace::Variables => Box::new(variables.variable_names()),
        };
        return names
            .filter(|name| name.starts_with(&string_context.typed))
            .map(|name| {
                variable_completion_item(
                    source,
                    string_context.replace_start,
                    offset,
                    name,
                    string_context.quote,
                )
            })
            .collect();
    }

    if lexed.ended_in_quoted_string {
        return Vec::new();
    }

    let Some(context) = member_completion_context(&lexed.tokens, offset) else {
        return Vec::new();
    };
    let specs = specs_for_path(&context.path, phase);

    specs
        .iter()
        .filter(|spec| spec.label.starts_with(&context.typed))
        .map(|spec| spec_completion_item(source, context.replace_start, offset, *spec))
        .collect()
}

fn specs_for_path(path: &[String], phase: ScriptEditorPhase) -> &'static [CompletionSpec] {
    match path {
        [] => GLOBALS,
        [api] if api == "api" => match phase {
            ScriptEditorPhase::PreRequest => ROOT_MEMBERS_PRE,
            ScriptEditorPhase::PostResponse => ROOT_MEMBERS_POST,
        },
        [api, request] if api == "api" && request == "request" => REQUEST_MEMBERS,
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
        [api, console] if api == "api" && console == "console" => CONSOLE_MEMBERS,
        [console] if console == "console" => CONSOLE_MEMBERS,
        _ => &[],
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

fn variable_completion_item(
    source: &str,
    replace_start: usize,
    offset: usize,
    name: &str,
    quote: char,
) -> CompletionItem {
    CompletionItem {
        label: name.to_owned(),
        kind: Some(CompletionItemKind::VARIABLE),
        detail: Some("active variable".to_owned()),
        documentation: Some(Documentation::String(
            "Variable name only; its value is never exposed to editor intelligence.".to_owned(),
        )),
        text_edit: Some(CompletionTextEdit::Edit(TextEdit {
            range: source_range(source, replace_start, offset),
            new_text: escape_for_quote(name, quote),
        })),
        ..Default::default()
    }
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
struct VariableStringCompletionContext {
    namespace: VariableNamespace,
    typed: String,
    replace_start: usize,
    quote: char,
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

fn source_range(source: &str, start: usize, end: usize) -> Range {
    Range::new(source_position(source, start), source_position(source, end))
}

// gpui-component currently treats an LSP Position column as a Unicode scalar
// index when mapping diagnostics and completion edits back into its Rope. Keep
// this conversion aligned with that editor contract.
fn source_position(source: &str, requested_offset: usize) -> Position {
    let offset = clipped_char_boundary(source, requested_offset);
    let prefix = &source[..offset];
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count() as u32;
    let column = prefix
        .rsplit_once('\n')
        .map_or(prefix, |(_, line)| line)
        .chars()
        .count() as u32;
    Position::new(line, column)
}

fn clipped_char_boundary(source: &str, requested_offset: usize) -> usize {
    let mut offset = requested_offset.min(source.len());
    while !source.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
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

    fn labels(items: Vec<CompletionItem>) -> Vec<String> {
        items.into_iter().map(|item| item.label).collect()
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
    fn catalog_is_value_blind_and_deterministic() {
        let catalog = ScriptVariableCatalog::from_names(
            ["zeta", "", "alpha", "zeta"],
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

        // Only names can be supplied or retained. A secret value is absent
        // from both the catalog and its Debug representation.
        assert!(!format!("{catalog:?}").contains("super-secret-value"));
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
    fn completion_offers_names_without_values() {
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
    fn diagnostics_warn_only_for_missing_literal_reads() {
        let source = r#"
api.environment.get("base_url");
api.environment.get("missing_env");
api.variables.get('collection_id');
api.variables.get('missing_anywhere');
"#;
        let diagnostics = diagnostics_for_source(source, &catalog());

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

        assert!(diagnostics_for_source(source, &catalog()).is_empty());
    }

    #[test]
    fn preceding_literal_set_satisfies_later_reads() {
        let source = r#"
api.environment.get("before");
api.environment.set("after", computeValue());
api.environment.get("after");
api.variables.get("after");
"#;
        let diagnostics = diagnostics_for_source(source, &catalog());

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

        let diagnostics = diagnostics_for_source(r#"api.environment.get("disabled")"#, &catalog);
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
        let diagnostics = diagnostics_for_source(source, &catalog());

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
        assert!(diagnostics_for_source(r#"api.environment.get("şehir")"#, &catalog).is_empty());
    }
}

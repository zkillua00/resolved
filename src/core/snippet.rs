#![allow(clippy::result_large_err)]
#![allow(dead_code)]

//! Persistable plain snippets and bounded JavaScript-backed snippet generators.
//!
//! Executable snippets run in a fresh QuickJS runtime. They receive copies of
//! the invocation context through a frozen `api` object and generator helpers
//! through a separate frozen `snippet` object. No browser, Node.js, filesystem,
//! network, environment-mutation, or request-mutation capability is installed.

use std::{
    collections::HashSet,
    fmt,
    io::{self, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use chrono::{DateTime, Utc};
use rquickjs::{
    CatchResultExt as _, CaughtError, Context as JsContext, Exception, Function,
    Runtime as JsRuntime, Value, context::EvalOptions,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{
    DbStringEnum,
    request::{BodyFieldKind, RequestDraft, ResponseData},
    template::redact_secret_values,
};

pub const SNIPPET_GENERATOR_API_VERSION: u32 = 1;
pub const SNIPPET_MEMORY_LIMIT_BYTES: usize = 32 * 1024 * 1024;
pub const SNIPPET_STACK_LIMIT_BYTES: usize = 256 * 1024;
pub const SNIPPET_TIMEOUT: Duration = Duration::from_millis(500);
pub const MAX_SNIPPET_SOURCE_BYTES: usize = 256 * 1024;
pub const MAX_SNIPPET_BODY_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_SNIPPET_SELECTION_BYTES: usize = 1024 * 1024;
pub const MAX_SNIPPET_CONTEXT_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_SNIPPET_CONTEXT_ROWS: usize = 4_096;
pub const MAX_SNIPPET_OUTPUT_BYTES: usize = 1024 * 1024;
pub const MAX_SNIPPET_LOG_ENTRIES: usize = 100;
pub const MAX_SNIPPET_LOG_BYTES: usize = 64 * 1024;
pub const MAX_SNIPPET_DIAGNOSTIC_MESSAGE_BYTES: usize = 16 * 1024;
pub const MAX_SNIPPET_DIAGNOSTIC_STACK_BYTES: usize = 64 * 1024;
const MAX_SNIPPET_REDACTION_CAPTURE_BYTES: usize = 1024 * 1024;

pub const MAX_SNIPPET_NAME_BYTES: usize = 256;
const MAX_SNIPPET_DESCRIPTION_BYTES: usize = 16 * 1024;
const MAX_SNIPPET_LANGUAGE_BYTES: usize = 64;
const MAX_JSON_SELECTION_DEPTH: usize = 256;
const MAX_JSON_SELECTION_NODES: usize = 200_000;
const SNIPPET_PRELUDE: &str = include_str!("snippet_runtime.js");
pub(crate) const GENERATOR_WRAPPER_PREFIX: &str = concat!(
    "(/** @returns {string | ResolvedSnippet.GeneratorReturn | void} */ ",
    "function () { \"use strict\"; ",
);
pub(crate) const GENERATOR_WRAPPER_SUFFIX: &str = "\n}).call(undefined)";

static NEXT_SNIPPET_ID: AtomicU64 = AtomicU64::new(0);

fn current_generator_api_version() -> u32 {
    SNIPPET_GENERATOR_API_VERSION
}

fn default_output_language() -> String {
    "javascript".to_owned()
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum SnippetKind {
    #[default]
    Plain,
    Executable,
}

impl SnippetKind {
    pub const fn all() -> &'static [Self] {
        &[Self::Plain, Self::Executable]
    }

    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::Plain => "plain",
            Self::Executable => "executable",
        }
    }

    pub fn from_db_str(value: &str) -> Option<Self> {
        Self::all()
            .iter()
            .copied()
            .find(|kind| kind.as_db_str() == value)
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Plain => "Plain",
            Self::Executable => "Executable",
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum SnippetCategory {
    PreRequest,
    PostResponse,
}

impl SnippetCategory {
    pub const fn all() -> &'static [Self] {
        &[Self::PreRequest, Self::PostResponse]
    }

    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::PreRequest => "pre_request",
            Self::PostResponse => "post_response",
        }
    }

    pub fn from_db_str(value: &str) -> Option<Self> {
        match value {
            "pre_request" | "pre-request" => Some(Self::PreRequest),
            "post_response" | "post-response" => Some(Self::PostResponse),
            _ => None,
        }
    }

    pub const fn engine_name(self) -> &'static str {
        match self {
            Self::PreRequest => "pre-request",
            Self::PostResponse => "post-response",
        }
    }

    pub const fn filename(self) -> &'static str {
        match self {
            Self::PreRequest => "pre-request-snippet.js",
            Self::PostResponse => "post-response-snippet.js",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::PreRequest => "Pre-request",
            Self::PostResponse => "Post-response",
        }
    }
}

impl fmt::Display for SnippetCategory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum SnippetRequirement {
    Always,
    HasRequest,
    HasResponse,
    HasSelection,
    HasRequestSelection,
    HasResponseSelection,
    HasJsonSelection,
}

impl SnippetRequirement {
    pub const fn all() -> &'static [Self] {
        &[
            Self::Always,
            Self::HasRequest,
            Self::HasResponse,
            Self::HasSelection,
            Self::HasRequestSelection,
            Self::HasResponseSelection,
            Self::HasJsonSelection,
        ]
    }

    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::Always => "all",
            Self::HasRequest => "has-request",
            Self::HasResponse => "has-response",
            Self::HasSelection => "has-selected-block",
            Self::HasRequestSelection => "has-request-selection",
            Self::HasResponseSelection => "has-response-selection",
            Self::HasJsonSelection => "has-json-selection",
        }
    }

    pub fn from_db_str(value: &str) -> Option<Self> {
        match value {
            "all" | "always" => Some(Self::Always),
            "has-request" | "has_request" => Some(Self::HasRequest),
            "has-response" | "has_response" => Some(Self::HasResponse),
            "has-selected-block" | "has-selection" | "has_selection" => Some(Self::HasSelection),
            "has-request-selection" | "has_request_selection" => Some(Self::HasRequestSelection),
            "has-response-selection" | "has_response_selection" => Some(Self::HasResponseSelection),
            "has-json-selection" | "has_json_selection" => Some(Self::HasJsonSelection),
            _ => None,
        }
    }

    pub const fn unmet_message(self) -> &'static str {
        match self {
            Self::Always => "",
            Self::HasRequest => "Requires a request.",
            Self::HasResponse => "Requires a current response.",
            Self::HasSelection => "Requires a selected block.",
            Self::HasRequestSelection => "Requires a selection from the request context.",
            Self::HasResponseSelection => "Requires a selection from the response context.",
            Self::HasJsonSelection => "Requires a selected JSON value.",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Always => "All contexts",
            Self::HasRequest => "Has request",
            Self::HasResponse => "Has response",
            Self::HasSelection => "Has selected block",
            Self::HasRequestSelection => "Has request selection",
            Self::HasResponseSelection => "Has response selection",
            Self::HasJsonSelection => "Has JSON selection",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Snippet {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub category: SnippetCategory,
    #[serde(default)]
    pub kind: SnippetKind,
    #[serde(default = "default_output_language")]
    pub output_language: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub requirements: Vec<SnippetRequirement>,
    #[serde(default = "current_generator_api_version")]
    pub generator_api_version: u32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl DbStringEnum for SnippetKind {
    fn from_db_str(value: &str) -> Option<Self> {
        Self::from_db_str(value)
    }
}

impl DbStringEnum for SnippetCategory {
    fn from_db_str(value: &str) -> Option<Self> {
        Self::from_db_str(value)
    }
}

impl DbStringEnum for SnippetRequirement {
    fn from_db_str(value: &str) -> Option<Self> {
        Self::from_db_str(value)
    }
}

impl Snippet {
    pub fn new(
        name: impl Into<String>,
        category: SnippetCategory,
        kind: SnippetKind,
    ) -> Result<Self, SnippetValidationError> {
        let now = Utc::now();
        let mut snippet = Self {
            id: new_snippet_id(),
            name: name.into().trim().to_owned(),
            description: String::new(),
            category,
            kind,
            output_language: default_output_language(),
            source: String::new(),
            requirements: Vec::new(),
            generator_api_version: SNIPPET_GENERATOR_API_VERSION,
            created_at: now,
            updated_at: now,
        };
        snippet.normalize();
        snippet.validate()?;
        Ok(snippet)
    }

    pub fn normalize(&mut self) {
        self.name = self.name.trim().to_owned();
        self.output_language = self.output_language.trim().to_ascii_lowercase();
        if self.requirements == [SnippetRequirement::Always] {
            self.requirements.clear();
        }
    }

    pub fn validate(&self) -> Result<(), SnippetValidationError> {
        validate_trimmed("snippet id", &self.id, 256)?;
        validate_trimmed("snippet name", &self.name, MAX_SNIPPET_NAME_BYTES)?;
        validate_trimmed(
            "snippet output language",
            &self.output_language,
            MAX_SNIPPET_LANGUAGE_BYTES,
        )?;
        if self.description.len() > MAX_SNIPPET_DESCRIPTION_BYTES {
            return Err(SnippetValidationError::FieldTooLong {
                field: "snippet description",
                size: self.description.len(),
                limit: MAX_SNIPPET_DESCRIPTION_BYTES,
            });
        }
        if self.source.len() > MAX_SNIPPET_SOURCE_BYTES {
            return Err(SnippetValidationError::SourceTooLarge {
                size: self.source.len(),
                limit: MAX_SNIPPET_SOURCE_BYTES,
            });
        }
        if self.generator_api_version != SNIPPET_GENERATOR_API_VERSION {
            return Err(SnippetValidationError::UnsupportedGeneratorApiVersion {
                found: self.generator_api_version,
                supported: SNIPPET_GENERATOR_API_VERSION,
            });
        }
        if self.updated_at < self.created_at {
            return Err(SnippetValidationError::UpdatedBeforeCreated);
        }
        let mut seen = HashSet::new();
        for requirement in &self.requirements {
            if !seen.insert(*requirement) {
                return Err(SnippetValidationError::DuplicateRequirement(*requirement));
            }
        }
        if seen.contains(&SnippetRequirement::Always) && seen.len() > 1 {
            return Err(SnippetValidationError::AlwaysCombinedWithRequirements);
        }
        Ok(())
    }

    pub fn availability(&self, context: &SnippetInvocationContext) -> SnippetAvailability {
        let mut reasons = Vec::new();
        if self.category != context.category {
            reasons.push(SnippetUnavailableReason::CategoryMismatch {
                expected: self.category,
                actual: context.category,
            });
        }
        for requirement in &self.requirements {
            if !requirement_is_met(*requirement, context) {
                reasons.push(SnippetUnavailableReason::RequirementNotMet(*requirement));
            }
        }
        SnippetAvailability { reasons }
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum SnippetValidationError {
    #[error("{field} cannot be empty")]
    EmptyField { field: &'static str },
    #[error("{field} must not have leading or trailing whitespace")]
    UntrimmedField { field: &'static str },
    #[error("{field} is {size} bytes; the limit is {limit} bytes")]
    FieldTooLong {
        field: &'static str,
        size: usize,
        limit: usize,
    },
    #[error("snippet source is {size} bytes; the limit is {limit} bytes")]
    SourceTooLarge { size: usize, limit: usize },
    #[error(
        "snippet generator API version {found} is not supported; this build supports {supported}"
    )]
    UnsupportedGeneratorApiVersion { found: u32, supported: u32 },
    #[error("snippet updated_at cannot precede created_at")]
    UpdatedBeforeCreated,
    #[error("snippet requirement '{}' is repeated", .0.as_db_str())]
    DuplicateRequirement(SnippetRequirement),
    #[error("the 'all' requirement cannot be combined with other requirements")]
    AlwaysCombinedWithRequirements,
}

fn validate_trimmed(
    field: &'static str,
    value: &str,
    limit: usize,
) -> Result<(), SnippetValidationError> {
    if value.trim().is_empty() {
        return Err(SnippetValidationError::EmptyField { field });
    }
    if value.trim() != value {
        return Err(SnippetValidationError::UntrimmedField { field });
    }
    if value.len() > limit {
        return Err(SnippetValidationError::FieldTooLong {
            field,
            size: value.len(),
            limit,
        });
    }
    Ok(())
}

fn new_snippet_id() -> String {
    let now = Utc::now();
    let sequence = NEXT_SNIPPET_ID.fetch_add(1, Ordering::Relaxed);
    format!(
        "snippet-{}-{}-{sequence}",
        now.timestamp_micros(),
        std::process::id()
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnippetAvailability {
    reasons: Vec<SnippetUnavailableReason>,
}

impl SnippetAvailability {
    pub fn is_available(&self) -> bool {
        self.reasons.is_empty()
    }

    pub fn reasons(&self) -> &[SnippetUnavailableReason] {
        &self.reasons
    }

    pub fn message(&self) -> Option<String> {
        (!self.reasons.is_empty()).then(|| {
            self.reasons
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(" ")
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SnippetUnavailableReason {
    CategoryMismatch {
        expected: SnippetCategory,
        actual: SnippetCategory,
    },
    RequirementNotMet(SnippetRequirement),
}

impl fmt::Display for SnippetUnavailableReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CategoryMismatch { expected, actual } => write!(
                formatter,
                "This is a {expected} snippet, but the {actual} editor invoked it."
            ),
            Self::RequirementNotMet(requirement) => {
                formatter.write_str(requirement.unmet_message())
            }
        }
    }
}

fn requirement_is_met(requirement: SnippetRequirement, context: &SnippetInvocationContext) -> bool {
    match requirement {
        SnippetRequirement::Always | SnippetRequirement::HasRequest => true,
        SnippetRequirement::HasResponse => {
            context.category == SnippetCategory::PostResponse && context.response.is_some()
        }
        SnippetRequirement::HasSelection => context.selection.is_some(),
        SnippetRequirement::HasRequestSelection => context
            .selection
            .as_ref()
            .is_some_and(|selection| matches!(selection.source, "pre-request" | "request")),
        SnippetRequirement::HasResponseSelection => context
            .selection
            .as_ref()
            .is_some_and(|selection| matches!(selection.source, "post-response" | "response")),
        SnippetRequirement::HasJsonSelection => context
            .selection
            .as_ref()
            .is_some_and(SnippetSelection::has_json_path),
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum SnippetSelectionSource {
    PreRequest,
    PostResponse,
    Request,
    Response,
}

impl SnippetSelectionSource {
    pub const fn engine_name(self) -> &'static str {
        match self {
            Self::PreRequest => "pre-request",
            Self::PostResponse => "post-response",
            Self::Request => "request",
            Self::Response => "response",
        }
    }

    pub const fn is_request(self) -> bool {
        matches!(self, Self::PreRequest | Self::Request)
    }

    pub const fn is_response(self) -> bool {
        matches!(self, Self::PostResponse | Self::Response)
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum SnippetSelectionArea {
    Script,
    Url,
    Headers,
    Body,
}

impl SnippetSelectionArea {
    pub const fn engine_name(self) -> &'static str {
        match self {
            Self::Script => "script",
            Self::Url => "url",
            Self::Headers => "headers",
            Self::Body => "body",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnippetTextRange {
    /// UTF-16 code-unit offset, matching JavaScript string indexing and GPUI's
    /// selected-text API.
    pub start: usize,
    /// Exclusive UTF-16 code-unit offset.
    pub end: usize,
}

impl SnippetTextRange {
    pub const fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    pub const fn is_empty(self) -> bool {
        self.start == self.end
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", content = "value", rename_all = "lowercase")]
pub enum JsonPathSegment {
    Key(String),
    Index(usize),
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct JsonPath {
    segments: Vec<JsonPathSegment>,
}

impl JsonPath {
    pub fn new(segments: Vec<JsonPathSegment>) -> Self {
        Self { segments }
    }

    pub fn segments(&self) -> &[JsonPathSegment] {
        &self.segments
    }

    pub fn json_pointer(&self) -> String {
        self.segments
            .iter()
            .fold(String::new(), |mut output, segment| {
                output.push('/');
                let value = match segment {
                    JsonPathSegment::Key(value) => value.clone(),
                    JsonPathSegment::Index(index) => index.to_string(),
                };
                output.push_str(&value.replace('~', "~0").replace('/', "~1"));
                output
            })
    }

    pub fn json_path(&self) -> String {
        format!("${}", self.property_suffix())
    }

    pub fn expression(&self, root: &str) -> Result<String, JsonSelectionError> {
        let root = root.trim();
        if root.is_empty() {
            return Err(JsonSelectionError::EmptyRootExpression);
        }
        Ok(format!("{root}{}", self.property_suffix()))
    }

    fn property_suffix(&self) -> String {
        self.segments
            .iter()
            .fold(String::new(), |mut output, segment| {
                match segment {
                    JsonPathSegment::Index(index) => output.push_str(&format!("[{index}]")),
                    JsonPathSegment::Key(key) if is_ascii_javascript_identifier(key) => {
                        output.push('.');
                        output.push_str(key);
                    }
                    JsonPathSegment::Key(key) => {
                        output.push('[');
                        output.push_str(
                            &serde_json::to_string(key)
                                .expect("serializing a JSON object key cannot fail"),
                        );
                        output.push(']');
                    }
                }
                output
            })
    }
}

fn is_ascii_javascript_identifier(value: &str) -> bool {
    let mut characters = value.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || matches!(first, '_' | '$'))
        && characters
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '$'))
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SnippetSelection {
    source: &'static str,
    area: &'static str,
    text: String,
    range: SnippetTextRange,
    content_type: Option<String>,
    json_path: Option<JsonPath>,
}

impl SnippetSelection {
    pub fn new(
        source: SnippetSelectionSource,
        area: SnippetSelectionArea,
        text: impl Into<String>,
        range: SnippetTextRange,
        content_type: Option<String>,
    ) -> Result<Self, JsonSelectionError> {
        let text = text.into();
        if range.start > range.end {
            return Err(JsonSelectionError::ReversedRange {
                start: range.start,
                end: range.end,
            });
        }
        if text.len() > MAX_SNIPPET_SELECTION_BYTES {
            return Err(JsonSelectionError::SelectionTooLarge {
                size: text.len(),
                limit: MAX_SNIPPET_SELECTION_BYTES,
            });
        }
        Ok(Self {
            source: source.engine_name(),
            area: area.engine_name(),
            text,
            range,
            content_type,
            json_path: None,
        })
    }

    pub fn from_json_document(
        source: SnippetSelectionSource,
        area: SnippetSelectionArea,
        document: &str,
        range: SnippetTextRange,
        content_type: Option<String>,
    ) -> Result<Self, JsonSelectionError> {
        if document.len() > MAX_SNIPPET_BODY_BYTES {
            return Err(JsonSelectionError::DocumentTooLarge {
                size: document.len(),
                limit: MAX_SNIPPET_BODY_BYTES,
            });
        }
        if range.start > range.end {
            return Err(JsonSelectionError::ReversedRange {
                start: range.start,
                end: range.end,
            });
        }
        let start = byte_offset_from_utf16(document, range.start).ok_or(
            JsonSelectionError::InvalidUtf16Offset {
                offset: range.start,
            },
        )?;
        let end = byte_offset_from_utf16(document, range.end)
            .ok_or(JsonSelectionError::InvalidUtf16Offset { offset: range.end })?;
        let selected_size = end.saturating_sub(start);
        if selected_size > MAX_SNIPPET_SELECTION_BYTES {
            return Err(JsonSelectionError::SelectionTooLarge {
                size: selected_size,
                limit: MAX_SNIPPET_SELECTION_BYTES,
            });
        }

        let target = trimmed_json_target(document, start, end)?;
        let path = JsonSpanParser::new(document, target).parse()?;
        let text = if start == end {
            document[target.start..target.end].to_owned()
        } else {
            document[start..end].to_owned()
        };
        Ok(Self {
            source: source.engine_name(),
            area: area.engine_name(),
            text,
            range,
            content_type,
            json_path: Some(path),
        })
    }

    pub fn source(&self) -> &str {
        self.source
    }

    pub fn area(&self) -> &str {
        self.area
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub const fn range(&self) -> SnippetTextRange {
        self.range
    }

    pub fn content_type(&self) -> Option<&str> {
        self.content_type.as_deref()
    }

    pub fn json_path(&self) -> Option<&JsonPath> {
        self.json_path.as_ref()
    }

    pub fn has_json_path(&self) -> bool {
        self.json_path.is_some()
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum JsonSelectionError {
    #[error("selection range starts at {start} but ends at {end}")]
    ReversedRange { start: usize, end: usize },
    #[error("UTF-16 offset {offset} is outside the document or splits a surrogate pair")]
    InvalidUtf16Offset { offset: usize },
    #[error("selected text is {size} bytes; the limit is {limit} bytes")]
    SelectionTooLarge { size: usize, limit: usize },
    #[error("JSON document is {size} bytes; the limit is {limit} bytes")]
    DocumentTooLarge { size: usize, limit: usize },
    #[error("the selected range contains only JSON whitespace")]
    WhitespaceOnly,
    #[error("invalid JSON at byte {offset}: {message}")]
    InvalidJson { offset: usize, message: String },
    #[error("JSON object repeats key '{key}' at byte {offset}; its path would be ambiguous")]
    DuplicateObjectKey { key: String, offset: usize },
    #[error("JSON nesting exceeds the {limit} level limit")]
    DepthLimit { limit: usize },
    #[error("JSON value count exceeds the {limit} node limit")]
    NodeLimit { limit: usize },
    #[error("the selected range does not map to a JSON value")]
    NoContainingValue,
    #[error("the root expression cannot be empty")]
    EmptyRootExpression,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnippetRequestHeader {
    enabled: bool,
    name: String,
    value: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnippetBodyField {
    enabled: bool,
    name: String,
    value: String,
    kind: &'static str,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnippetRequest {
    method: String,
    url: String,
    headers: Vec<SnippetRequestHeader>,
    body: String,
    body_mode: &'static str,
    raw_body_language: &'static str,
    body_fields: Vec<SnippetBodyField>,
    body_truncated: bool,
}

impl SnippetRequest {
    fn from_request(request: &RequestDraft, budget: &mut SnippetContextBudget) -> Self {
        let (body, body_was_truncated) =
            budget.copy_bounded("request body", &request.body, MAX_SNIPPET_BODY_BYTES);
        let body_truncated = body_was_truncated;
        let body_fields = if budget.reserve_rows("request body", request.body_fields.len()) {
            request
                .body_fields
                .iter()
                .map(|field| {
                    let name = budget.copy("request body-field name", &field.name);
                    let value = budget.copy("request body-field value", &field.value);
                    SnippetBodyField {
                        enabled: field.enabled,
                        name,
                        value,
                        kind: match field.kind {
                            BodyFieldKind::Text => "text",
                            BodyFieldKind::File => "file",
                        },
                    }
                })
                .collect()
        } else {
            Vec::new()
        };
        let headers = if budget.reserve_rows("request header", request.headers.len()) {
            request
                .headers
                .iter()
                .map(|header| SnippetRequestHeader {
                    enabled: header.enabled,
                    name: budget.copy("request header name", &header.name),
                    value: budget.copy("request header value", &header.value),
                })
                .collect()
        } else {
            Vec::new()
        };
        Self {
            method: budget.copy("request method", &request.method),
            url: budget.copy("request URL", &request.url),
            headers,
            body,
            body_mode: request.body_mode.as_db_str(),
            raw_body_language: request.raw_body_language.as_db_str(),
            body_fields,
            body_truncated,
        }
    }

    pub fn body_truncated(&self) -> bool {
        self.body_truncated
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnippetResponseHeader {
    enabled: bool,
    name: String,
    value: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnippetResponse {
    status: u16,
    status_text: String,
    http_version: String,
    url: String,
    content_type: Option<String>,
    headers: Vec<SnippetResponseHeader>,
    duration_ms: u64,
    size_bytes: usize,
    body_text: String,
    body_truncated: bool,
}

impl SnippetResponse {
    fn from_response(response: &ResponseData, budget: &mut SnippetContextBudget) -> Self {
        let visible_len = response.body.len().min(MAX_SNIPPET_BODY_BYTES);
        let visible_body = &response.body[..visible_len];
        let lossy_len = utf8_lossy_len(visible_body);
        let body_text = if budget.reserve_bytes("response body", lossy_len) {
            String::from_utf8_lossy(visible_body).into_owned()
        } else {
            String::new()
        };
        let headers = if budget.reserve_rows("response header", response.headers.len()) {
            response
                .headers
                .iter()
                .map(|header| SnippetResponseHeader {
                    enabled: true,
                    name: budget.copy("response header name", &header.name),
                    value: budget.copy("response header value", &header.value),
                })
                .collect()
        } else {
            Vec::new()
        };
        Self {
            status: response.status,
            status_text: budget.copy("response status text", &response.status_text),
            http_version: budget.copy("response HTTP version", &response.http_version),
            url: budget.copy("response URL", &response.final_url),
            content_type: response
                .content_type
                .as_deref()
                .map(|value| budget.copy("response content type", value)),
            headers,
            duration_ms: response.duration.as_millis().min(u128::from(u64::MAX)) as u64,
            size_bytes: response.body.len(),
            body_text,
            body_truncated: response.body.len() > visible_len,
        }
    }

    pub fn body_truncated(&self) -> bool {
        self.body_truncated
    }
}

#[derive(Clone, Debug)]
pub struct SnippetInvocationContext {
    category: SnippetCategory,
    request: SnippetRequest,
    response: Option<SnippetResponse>,
    selection: Option<SnippetSelection>,
    sensitive_values: Vec<String>,
    resource_budget: SnippetContextBudget,
}

impl SnippetInvocationContext {
    pub fn new(category: SnippetCategory, request: &RequestDraft) -> Self {
        let mut resource_budget = SnippetContextBudget::new();
        let request = SnippetRequest::from_request(request, &mut resource_budget);
        Self {
            category,
            request,
            response: None,
            selection: None,
            sensitive_values: Vec::new(),
            resource_budget,
        }
    }

    pub fn with_response(mut self, response: &ResponseData) -> Self {
        self.response = Some(SnippetResponse::from_response(
            response,
            &mut self.resource_budget,
        ));
        self
    }

    pub fn with_selection(mut self, selection: SnippetSelection) -> Self {
        self.resource_budget
            .reserve_bytes("selection text", selection.text.len());
        if let Some(content_type) = selection.content_type.as_deref() {
            self.resource_budget
                .reserve_bytes("selection content type", content_type.len());
        }
        if let Some(path) = selection.json_path.as_ref() {
            self.resource_budget
                .reserve_rows("selection path", path.segments.len());
            for segment in &path.segments {
                if let JsonPathSegment::Key(key) = segment {
                    self.resource_budget
                        .reserve_bytes("selection path key", key.len());
                }
            }
        }
        self.selection = Some(selection);
        self
    }

    pub fn with_sensitive_values<I, S>(mut self, values: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.sensitive_values.clear();
        for value in values {
            let value = value.as_ref();
            if value.is_empty() {
                continue;
            }
            if !self.resource_budget.reserve_rows("sensitive value", 1) {
                break;
            }
            if !self
                .resource_budget
                .reserve_bytes("sensitive redaction value", value.len())
            {
                break;
            }
            self.sensitive_values.push(value.to_owned());
        }
        self
    }

    pub const fn category(&self) -> SnippetCategory {
        self.category
    }

    pub fn request(&self) -> &SnippetRequest {
        &self.request
    }

    pub fn response(&self) -> Option<&SnippetResponse> {
        self.response.as_ref()
    }

    pub fn selection(&self) -> Option<&SnippetSelection> {
        self.selection.as_ref()
    }

    fn scrub(&self, text: impl AsRef<str>) -> String {
        redact_secret_values(text.as_ref(), &self.sensitive_values)
    }

    fn scrub_bounded(&self, text: impl AsRef<str>, max_bytes: usize) -> String {
        let text = text.as_ref();
        if self.sensitive_values.is_empty() {
            return bounded_str(text, max_bytes).0.to_owned();
        }
        let Some(capture_bytes) = self.redaction_capture_bytes(max_bytes) else {
            let mut marker = "[REDACTED]".to_owned();
            truncate_utf8(&mut marker, max_bytes);
            return marker;
        };
        let captured = bounded_str(text, capture_bytes).0;
        let mut scrubbed = self.scrub(captured);
        truncate_utf8(&mut scrubbed, max_bytes);
        scrubbed
    }

    fn log_capture_characters(&self) -> usize {
        self.redaction_capture_bytes(MAX_SNIPPET_LOG_BYTES)
            .unwrap_or(0)
    }

    fn redaction_capture_bytes(&self, output_bytes: usize) -> Option<usize> {
        let lookahead = self
            .sensitive_values
            .iter()
            .map(String::len)
            .max()
            .unwrap_or(0)
            .checked_mul(3)?;
        output_bytes
            .checked_add(lookahead)?
            .checked_sub(lookahead.min(1))
            .filter(|capture| *capture <= MAX_SNIPPET_REDACTION_CAPTURE_BYTES)
    }

    fn resource_error(&self) -> Option<&str> {
        self.resource_budget.error.as_deref()
    }
}

#[derive(Clone, Debug)]
struct SnippetContextBudget {
    remaining_bytes: usize,
    remaining_rows: usize,
    error: Option<String>,
}

impl SnippetContextBudget {
    fn new() -> Self {
        Self {
            remaining_bytes: MAX_SNIPPET_CONTEXT_BYTES,
            remaining_rows: MAX_SNIPPET_CONTEXT_ROWS,
            error: None,
        }
    }

    fn copy(&mut self, label: &'static str, value: &str) -> String {
        if self.reserve_bytes(label, value.len()) {
            value.to_owned()
        } else {
            String::new()
        }
    }

    fn copy_bounded(
        &mut self,
        label: &'static str,
        value: &str,
        max_bytes: usize,
    ) -> (String, bool) {
        let (value, truncated) = bounded_str(value, max_bytes);
        (self.copy(label, value), truncated)
    }

    fn reserve_bytes(&mut self, label: &'static str, bytes: usize) -> bool {
        if self.error.is_some() {
            return false;
        }
        let Some(remaining) = self.remaining_bytes.checked_sub(bytes) else {
            self.error = Some(format!(
                "snippet context exceeds its {MAX_SNIPPET_CONTEXT_BYTES} byte resource limit while copying {label}"
            ));
            return false;
        };
        self.remaining_bytes = remaining;
        true
    }

    fn reserve_rows(&mut self, label: &'static str, rows: usize) -> bool {
        if self.error.is_some() {
            return false;
        }
        let Some(remaining) = self.remaining_rows.checked_sub(rows) else {
            self.error = Some(format!(
                "snippet context exceeds its {MAX_SNIPPET_CONTEXT_ROWS} combined-row limit at {label} rows"
            ));
            return false;
        };
        self.remaining_rows = remaining;
        true
    }
}

fn bounded_str(value: &str, max_bytes: usize) -> (&str, bool) {
    if value.len() <= max_bytes {
        return (value, false);
    }
    let mut boundary = max_bytes;
    while boundary > 0 && !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    (&value[..boundary], true)
}

fn utf8_lossy_len(mut bytes: &[u8]) -> usize {
    let mut length = 0usize;
    loop {
        match std::str::from_utf8(bytes) {
            Ok(valid) => return length.saturating_add(valid.len()),
            Err(error) => {
                length = length
                    .saturating_add(error.valid_up_to())
                    .saturating_add('\u{FFFD}'.len_utf8());
                let consumed = error
                    .valid_up_to()
                    .saturating_add(error.error_len().unwrap_or(bytes.len()));
                if consumed >= bytes.len() {
                    return length;
                }
                bytes = &bytes[consumed..];
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct SnippetCancellation {
    cancelled: Arc<AtomicBool>,
}

impl SnippetCancellation {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SnippetLogLevel {
    Log,
    Info,
    Warn,
    Error,
    Debug,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct SnippetLog {
    pub level: SnippetLogLevel,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct SnippetReport {
    pub duration: Duration,
    pub logs: Vec<SnippetLog>,
    pub request_body_truncated: bool,
    pub response_body_truncated: bool,
}

impl SnippetReport {
    fn empty(context: &SnippetInvocationContext) -> Self {
        Self {
            duration: Duration::ZERO,
            logs: Vec::new(),
            request_body_truncated: context.request.body_truncated(),
            response_body_truncated: context
                .response
                .as_ref()
                .is_some_and(SnippetResponse::body_truncated),
        }
    }
}

#[derive(Clone, Debug)]
pub struct GeneratedSnippet {
    pub text: String,
    /// UTF-16 code-unit offset within `text`.
    pub cursor: Option<usize>,
    pub report: SnippetReport,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnippetErrorKind {
    InvalidDefinition,
    Unavailable,
    SourceLimit,
    ContextLimit,
    Cancelled,
    TimedOut,
    MemoryLimit,
    Syntax,
    Runtime,
    InvalidOutput,
    OutputLimit,
    Engine,
}

#[derive(Clone, Debug)]
pub struct SnippetDiagnostic {
    pub category: SnippetCategory,
    pub kind: SnippetErrorKind,
    pub filename: &'static str,
    pub message: String,
    pub stack: Option<String>,
}

#[derive(Clone, Debug)]
pub struct SnippetError {
    pub diagnostic: SnippetDiagnostic,
    pub report: SnippetReport,
}

impl fmt::Display for SnippetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "snippet generation failed: {}",
            self.diagnostic.message
        )
    }
}

impl std::error::Error for SnippetError {}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EngineInput<'a> {
    api_version: u32,
    category: &'static str,
    output_language: &'a str,
    request: &'a SnippetRequest,
    response: Option<&'a SnippetResponse>,
    selection: Option<&'a SnippetSelection>,
    max_log_entries: usize,
    max_log_characters: usize,
    max_output_characters: usize,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct EngineOutput {
    text: String,
    cursor: Option<usize>,
    logs: Vec<SnippetLog>,
}

#[derive(Deserialize)]
struct EnginePartialOutput {
    logs: Vec<SnippetLog>,
}

struct ContextSizeWriter {
    bytes: usize,
    exceeded: bool,
}

impl ContextSizeWriter {
    fn new() -> Self {
        Self {
            bytes: 0,
            exceeded: false,
        }
    }
}

impl Write for ContextSizeWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let next = self.bytes.saturating_add(buffer.len());
        if next > MAX_SNIPPET_CONTEXT_BYTES {
            self.exceeded = true;
            return Err(io::Error::other("snippet context size limit exceeded"));
        }
        self.bytes = next;
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub fn generate_snippet(
    snippet: &Snippet,
    context: &SnippetInvocationContext,
    cancellation: &SnippetCancellation,
) -> Result<GeneratedSnippet, SnippetError> {
    if snippet.source.len() > MAX_SNIPPET_SOURCE_BYTES {
        return Err(simple_error(
            snippet.category,
            SnippetErrorKind::SourceLimit,
            format!(
                "snippet source is {} bytes; the limit is {MAX_SNIPPET_SOURCE_BYTES} bytes",
                snippet.source.len()
            ),
            context,
        ));
    }
    snippet.validate().map_err(|error| {
        simple_error(
            snippet.category,
            SnippetErrorKind::InvalidDefinition,
            error.to_string(),
            context,
        )
    })?;
    let availability = snippet.availability(context);
    if !availability.is_available() {
        return Err(simple_error(
            snippet.category,
            SnippetErrorKind::Unavailable,
            availability
                .message()
                .unwrap_or_else(|| "snippet is not available in this context".to_owned()),
            context,
        ));
    }
    if cancellation.is_cancelled() {
        return Err(simple_error(
            snippet.category,
            SnippetErrorKind::Cancelled,
            "snippet generation cancelled",
            context,
        ));
    }

    match snippet.kind {
        SnippetKind::Plain => Ok(GeneratedSnippet {
            text: snippet.source.clone(),
            cursor: None,
            report: SnippetReport::empty(context),
        }),
        SnippetKind::Executable => execute_generator(snippet, context, cancellation),
    }
}

fn execute_generator(
    snippet: &Snippet,
    context: &SnippetInvocationContext,
    cancellation: &SnippetCancellation,
) -> Result<GeneratedSnippet, SnippetError> {
    let started = Instant::now();
    let deadline = started + SNIPPET_TIMEOUT;
    if cancellation.is_cancelled() {
        return Err(engine_error(
            snippet.category,
            SnippetErrorKind::Cancelled,
            "snippet generation cancelled",
            started.elapsed(),
            context,
            Vec::new(),
        ));
    }
    if let Some(message) = context.resource_error() {
        return Err(engine_error(
            snippet.category,
            SnippetErrorKind::ContextLimit,
            message,
            started.elapsed(),
            context,
            Vec::new(),
        ));
    }
    let response = (snippet.category == SnippetCategory::PostResponse)
        .then_some(context.response.as_ref())
        .flatten();
    let input = EngineInput {
        api_version: snippet.generator_api_version,
        category: snippet.category.engine_name(),
        output_language: &snippet.output_language,
        request: &context.request,
        response,
        selection: context.selection.as_ref(),
        max_log_entries: MAX_SNIPPET_LOG_ENTRIES,
        max_log_characters: context.log_capture_characters(),
        max_output_characters: MAX_SNIPPET_OUTPUT_BYTES,
    };
    let timed_out = Arc::new(AtomicBool::new(false));
    let mut context_size = ContextSizeWriter::new();
    let measured = serde_json::to_writer(&mut context_size, &input);
    ensure_not_interrupted(
        snippet.category,
        started,
        deadline,
        context,
        cancellation,
        &timed_out,
    )?;
    if context_size.exceeded {
        return Err(engine_error(
            snippet.category,
            SnippetErrorKind::ContextLimit,
            format!(
                "snippet context exceeds its {MAX_SNIPPET_CONTEXT_BYTES} byte serialized limit"
            ),
            started.elapsed(),
            context,
            Vec::new(),
        ));
    }
    if let Err(error) = measured {
        return Err(engine_error(
            snippet.category,
            SnippetErrorKind::Engine,
            format!("could not measure snippet context: {error}"),
            started.elapsed(),
            context,
            Vec::new(),
        ));
    }

    let timed_out_for_interrupt = Arc::clone(&timed_out);
    let cancelled_for_interrupt = Arc::clone(&cancellation.cancelled);

    let runtime = JsRuntime::new().map_err(|error| {
        classified_engine_error(
            snippet.category,
            SnippetErrorKind::Engine,
            error.to_string(),
            started,
            deadline,
            context,
            cancellation,
            &timed_out,
            Vec::new(),
        )
    })?;
    ensure_not_interrupted(
        snippet.category,
        started,
        deadline,
        context,
        cancellation,
        &timed_out,
    )?;
    runtime.set_memory_limit(SNIPPET_MEMORY_LIMIT_BYTES);
    runtime.set_max_stack_size(SNIPPET_STACK_LIMIT_BYTES);
    runtime.set_interrupt_handler(Some(Box::new(move || {
        if cancelled_for_interrupt.load(Ordering::Acquire) {
            return true;
        }
        if Instant::now() >= deadline {
            timed_out_for_interrupt.store(true, Ordering::Release);
            return true;
        }
        false
    })));

    let js_context = JsContext::full(&runtime).map_err(|error| {
        classified_engine_error(
            snippet.category,
            SnippetErrorKind::Engine,
            error.to_string(),
            started,
            deadline,
            context,
            cancellation,
            &timed_out,
            Vec::new(),
        )
    })?;
    ensure_not_interrupted(
        snippet.category,
        started,
        deadline,
        context,
        cancellation,
        &timed_out,
    )?;

    let generated = js_context.with(|ctx| {
        ensure_not_interrupted(
            snippet.category,
            started,
            deadline,
            context,
            cancellation,
            &timed_out,
        )?;
        let input_value = rquickjs_serde::to_value(ctx.clone(), &input).map_err(|error| {
            classified_engine_error(
                snippet.category,
                SnippetErrorKind::Engine,
                error.to_string(),
                started,
                deadline,
                context,
                cancellation,
                &timed_out,
                Vec::new(),
            )
        })?;
        ensure_not_interrupted(
            snippet.category,
            started,
            deadline,
            context,
            cancellation,
            &timed_out,
        )?;
        ctx.globals()
            .set("__RESOLVED_SNIPPET_INPUT", input_value)
            .map_err(|error| {
                classified_engine_error(
                    snippet.category,
                    SnippetErrorKind::Engine,
                    error.to_string(),
                    started,
                    deadline,
                    context,
                    cancellation,
                    &timed_out,
                    Vec::new(),
                )
            })?;
        ensure_not_interrupted(
            snippet.category,
            started,
            deadline,
            context,
            cancellation,
            &timed_out,
        )?;
        if let Err(caught) = ctx.eval::<(), _>(SNIPPET_PRELUDE).catch(&ctx) {
            return Err(caught_runtime_error(
                snippet.category,
                caught,
                started,
                deadline,
                context,
                cancellation,
                &timed_out,
                Vec::new(),
            ));
        }
        ensure_not_interrupted(
            snippet.category,
            started,
            deadline,
            context,
            cancellation,
            &timed_out,
        )?;

        let mut wrapped = String::with_capacity(
            GENERATOR_WRAPPER_PREFIX.len() + snippet.source.len() + GENERATOR_WRAPPER_SUFFIX.len(),
        );
        wrapped.push_str(GENERATOR_WRAPPER_PREFIX);
        wrapped.push_str(&snippet.source);
        wrapped.push_str(GENERATOR_WRAPPER_SUFFIX);
        let mut options = EvalOptions::default();
        options.global = true;
        options.strict = true;
        options.backtrace_barrier = true;
        options.promise = false;
        options.filename = Some(snippet.category.filename().to_owned());

        let returned = match ctx
            .eval_with_options::<Value<'_>, _>(wrapped, options)
            .catch(&ctx)
        {
            Ok(value) => value,
            Err(caught) => {
                return Err(caught_runtime_error(
                    snippet.category,
                    caught,
                    started,
                    deadline,
                    context,
                    cancellation,
                    &timed_out,
                    partial_logs(&ctx),
                ));
            }
        };
        ensure_not_interrupted(
            snippet.category,
            started,
            deadline,
            context,
            cancellation,
            &timed_out,
        )?;

        let finish: Function<'_> =
            ctx.globals()
                .get("__RESOLVED_SNIPPET_FINISH")
                .map_err(|error| {
                    classified_engine_error(
                        snippet.category,
                        SnippetErrorKind::Engine,
                        format!("could not access snippet result collector: {error}"),
                        started,
                        deadline,
                        context,
                        cancellation,
                        &timed_out,
                        partial_logs(&ctx),
                    )
                })?;
        ensure_not_interrupted(
            snippet.category,
            started,
            deadline,
            context,
            cancellation,
            &timed_out,
        )?;
        let value = match finish.call::<_, Value<'_>>((returned,)).catch(&ctx) {
            Ok(value) => value,
            Err(caught) => {
                return Err(caught_output_error(
                    snippet.category,
                    caught,
                    started,
                    deadline,
                    context,
                    cancellation,
                    &timed_out,
                    partial_logs(&ctx),
                ));
            }
        };
        ensure_not_interrupted(
            snippet.category,
            started,
            deadline,
            context,
            cancellation,
            &timed_out,
        )?;
        let output: EngineOutput = rquickjs_serde::from_value_strict(value).map_err(|error| {
            classified_engine_error(
                snippet.category,
                SnippetErrorKind::InvalidOutput,
                format!("snippet generator produced invalid output: {error}"),
                started,
                deadline,
                context,
                cancellation,
                &timed_out,
                partial_logs(&ctx),
            )
        })?;
        ensure_not_interrupted(
            snippet.category,
            started,
            deadline,
            context,
            cancellation,
            &timed_out,
        )?;
        generated_from_engine(snippet.category, output, started.elapsed(), context)
    })?;
    ensure_not_interrupted(
        snippet.category,
        started,
        deadline,
        context,
        cancellation,
        &timed_out,
    )?;
    Ok(generated)
}

fn generated_from_engine(
    category: SnippetCategory,
    output: EngineOutput,
    duration: Duration,
    context: &SnippetInvocationContext,
) -> Result<GeneratedSnippet, SnippetError> {
    if output.text.len() > MAX_SNIPPET_OUTPUT_BYTES {
        return Err(engine_error(
            category,
            SnippetErrorKind::OutputLimit,
            format!(
                "generated snippet is {} bytes; the limit is {MAX_SNIPPET_OUTPUT_BYTES} bytes",
                output.text.len()
            ),
            duration,
            context,
            output.logs,
        ));
    }
    if let Some(cursor) = output.cursor
        && byte_offset_from_utf16(&output.text, cursor).is_none()
    {
        let text_len_utf16 = output.text.encode_utf16().count();
        return Err(engine_error(
            category,
            SnippetErrorKind::InvalidOutput,
            format!(
                "snippet cursor {cursor} is outside the generated text or splits a surrogate pair ({text_len_utf16} UTF-16 code units)"
            ),
            duration,
            context,
            output.logs,
        ));
    }
    Ok(GeneratedSnippet {
        text: output.text,
        cursor: output.cursor,
        report: report_from_logs(output.logs, duration, context),
    })
}

fn partial_logs(ctx: &rquickjs::Ctx<'_>) -> Vec<SnippetLog> {
    ctx.eval::<Value<'_>, _>("__RESOLVED_SNIPPET_PARTIAL()")
        .ok()
        .and_then(|value| rquickjs_serde::from_value_strict::<EnginePartialOutput>(value).ok())
        .map(|output| output.logs)
        .unwrap_or_default()
}

fn report_from_logs(
    logs: Vec<SnippetLog>,
    duration: Duration,
    context: &SnippetInvocationContext,
) -> SnippetReport {
    let mut used_bytes = 0;
    let logs = logs
        .into_iter()
        .take(MAX_SNIPPET_LOG_ENTRIES)
        .filter_map(|log| {
            if used_bytes >= MAX_SNIPPET_LOG_BYTES {
                return None;
            }
            let remaining = MAX_SNIPPET_LOG_BYTES - used_bytes;
            let message = context.scrub_bounded(log.message, remaining);
            used_bytes += message.len();
            Some(SnippetLog {
                level: log.level,
                message,
            })
        })
        .collect();
    SnippetReport {
        duration,
        logs,
        request_body_truncated: context.request.body_truncated(),
        response_body_truncated: context
            .response
            .as_ref()
            .is_some_and(SnippetResponse::body_truncated),
    }
}

fn truncate_utf8(value: &mut String, max_bytes: usize) {
    if value.len() <= max_bytes {
        return;
    }
    let mut boundary = max_bytes;
    while boundary > 0 && !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value.truncate(boundary);
}

fn simple_error(
    category: SnippetCategory,
    kind: SnippetErrorKind,
    message: impl AsRef<str>,
    context: &SnippetInvocationContext,
) -> SnippetError {
    engine_error(category, kind, message, Duration::ZERO, context, Vec::new())
}

fn engine_error(
    category: SnippetCategory,
    kind: SnippetErrorKind,
    message: impl AsRef<str>,
    duration: Duration,
    context: &SnippetInvocationContext,
    logs: Vec<SnippetLog>,
) -> SnippetError {
    SnippetError {
        diagnostic: SnippetDiagnostic {
            category,
            kind,
            filename: category.filename(),
            message: context.scrub_bounded(message, MAX_SNIPPET_DIAGNOSTIC_MESSAGE_BYTES),
            stack: None,
        },
        report: report_from_logs(logs, duration, context),
    }
}

fn interruption_kind(
    cancellation: &SnippetCancellation,
    timed_out: &AtomicBool,
    deadline: Instant,
) -> Option<SnippetErrorKind> {
    if cancellation.is_cancelled() {
        return Some(SnippetErrorKind::Cancelled);
    }
    if timed_out.load(Ordering::Acquire) || Instant::now() >= deadline {
        timed_out.store(true, Ordering::Release);
        return Some(SnippetErrorKind::TimedOut);
    }
    None
}

fn interrupted_error(
    category: SnippetCategory,
    kind: SnippetErrorKind,
    duration: Duration,
    context: &SnippetInvocationContext,
    logs: Vec<SnippetLog>,
) -> SnippetError {
    let message = match kind {
        SnippetErrorKind::Cancelled => "snippet generation cancelled".to_owned(),
        SnippetErrorKind::TimedOut => format!(
            "snippet generator exceeded its {} ms execution limit",
            SNIPPET_TIMEOUT.as_millis()
        ),
        _ => unreachable!("only cancellation and timeout are interruption kinds"),
    };
    engine_error(category, kind, message, duration, context, logs)
}

fn ensure_not_interrupted(
    category: SnippetCategory,
    started: Instant,
    deadline: Instant,
    context: &SnippetInvocationContext,
    cancellation: &SnippetCancellation,
    timed_out: &AtomicBool,
) -> Result<(), SnippetError> {
    match interruption_kind(cancellation, timed_out, deadline) {
        Some(kind) => Err(interrupted_error(
            category,
            kind,
            started.elapsed(),
            context,
            Vec::new(),
        )),
        None => Ok(()),
    }
}

#[allow(clippy::too_many_arguments)]
fn classified_engine_error(
    category: SnippetCategory,
    fallback_kind: SnippetErrorKind,
    message: impl AsRef<str>,
    started: Instant,
    deadline: Instant,
    context: &SnippetInvocationContext,
    cancellation: &SnippetCancellation,
    timed_out: &AtomicBool,
    logs: Vec<SnippetLog>,
) -> SnippetError {
    if let Some(kind) = interruption_kind(cancellation, timed_out, deadline) {
        interrupted_error(category, kind, started.elapsed(), context, logs)
    } else {
        engine_error(
            category,
            fallback_kind,
            message,
            started.elapsed(),
            context,
            logs,
        )
    }
}

#[allow(clippy::too_many_arguments)]
fn caught_runtime_error(
    category: SnippetCategory,
    caught: CaughtError<'_>,
    started: Instant,
    deadline: Instant,
    context: &SnippetInvocationContext,
    cancellation: &SnippetCancellation,
    timed_out: &AtomicBool,
    logs: Vec<SnippetLog>,
) -> SnippetError {
    if let Some(kind) = interruption_kind(cancellation, timed_out, deadline) {
        return interrupted_error(category, kind, started.elapsed(), context, logs);
    }
    let error = caught_error(
        category,
        caught,
        started.elapsed(),
        context,
        logs,
        SnippetErrorKind::Runtime,
    );
    if let Some(kind) = interruption_kind(cancellation, timed_out, deadline) {
        interrupted_error(
            category,
            kind,
            started.elapsed(),
            context,
            error.report.logs,
        )
    } else {
        error
    }
}

#[allow(clippy::too_many_arguments)]
fn caught_output_error(
    category: SnippetCategory,
    caught: CaughtError<'_>,
    started: Instant,
    deadline: Instant,
    context: &SnippetInvocationContext,
    cancellation: &SnippetCancellation,
    timed_out: &AtomicBool,
    logs: Vec<SnippetLog>,
) -> SnippetError {
    if let Some(kind) = interruption_kind(cancellation, timed_out, deadline) {
        return interrupted_error(category, kind, started.elapsed(), context, logs);
    }
    let error = caught_error(
        category,
        caught,
        started.elapsed(),
        context,
        logs,
        SnippetErrorKind::InvalidOutput,
    );
    if let Some(kind) = interruption_kind(cancellation, timed_out, deadline) {
        interrupted_error(
            category,
            kind,
            started.elapsed(),
            context,
            error.report.logs,
        )
    } else {
        error
    }
}

fn caught_error(
    category: SnippetCategory,
    caught: CaughtError<'_>,
    duration: Duration,
    context: &SnippetInvocationContext,
    logs: Vec<SnippetLog>,
    fallback_kind: SnippetErrorKind,
) -> SnippetError {
    match caught {
        CaughtError::Error(rquickjs::Error::Allocation) => engine_error(
            category,
            SnippetErrorKind::MemoryLimit,
            "snippet generator exceeded its memory limit",
            duration,
            context,
            logs,
        ),
        CaughtError::Error(error) => engine_error(
            category,
            fallback_kind,
            error.to_string(),
            duration,
            context,
            logs,
        ),
        CaughtError::Exception(exception) => {
            exception_error(category, exception, duration, context, logs, fallback_kind)
        }
        CaughtError::Value(value) => {
            let message = value
                .as_string()
                .and_then(|value| value.to_string().ok())
                .unwrap_or_else(|| format!("JavaScript threw a {}", value.type_name()));
            engine_error(category, fallback_kind, message, duration, context, logs)
        }
    }
}

fn exception_error(
    category: SnippetCategory,
    exception: Exception<'_>,
    duration: Duration,
    context: &SnippetInvocationContext,
    logs: Vec<SnippetLog>,
    fallback_kind: SnippetErrorKind,
) -> SnippetError {
    let name = exception
        .as_object()
        .get::<_, String>("name")
        .unwrap_or_else(|_| "Error".to_owned());
    let message = exception.message().unwrap_or_else(|| name.clone());
    let kind = if name == "SyntaxError" {
        SnippetErrorKind::Syntax
    } else if name == "SnippetOutputLimitError" {
        SnippetErrorKind::OutputLimit
    } else if message
        .as_bytes()
        .windows(b"out of memory".len())
        .any(|window| window.eq_ignore_ascii_case(b"out of memory"))
    {
        SnippetErrorKind::MemoryLimit
    } else {
        fallback_kind
    };
    let stack = exception
        .stack()
        .map(|stack| context.scrub_bounded(stack, MAX_SNIPPET_DIAGNOSTIC_STACK_BYTES));
    SnippetError {
        diagnostic: SnippetDiagnostic {
            category,
            kind,
            filename: category.filename(),
            message: context.scrub_bounded(message, MAX_SNIPPET_DIAGNOSTIC_MESSAGE_BYTES),
            stack,
        },
        report: report_from_logs(logs, duration, context),
    }
}

#[derive(Clone, Copy)]
struct JsonTarget {
    start: usize,
    end: usize,
}

fn byte_offset_from_utf16(value: &str, target: usize) -> Option<usize> {
    let mut utf16 = 0;
    for (byte, character) in value.char_indices() {
        if utf16 == target {
            return Some(byte);
        }
        utf16 += character.len_utf16();
        if utf16 > target {
            return None;
        }
    }
    (utf16 == target).then_some(value.len())
}

fn trimmed_json_target(
    document: &str,
    start: usize,
    end: usize,
) -> Result<JsonTarget, JsonSelectionError> {
    if start == end {
        return Ok(JsonTarget { start, end });
    }
    let bytes = document.as_bytes();
    let mut trimmed_start = start;
    let mut trimmed_end = end;
    while trimmed_start < trimmed_end && is_json_whitespace(bytes[trimmed_start]) {
        trimmed_start += 1;
    }
    while trimmed_end > trimmed_start && is_json_whitespace(bytes[trimmed_end - 1]) {
        trimmed_end -= 1;
    }
    if trimmed_start == trimmed_end {
        return Err(JsonSelectionError::WhitespaceOnly);
    }
    Ok(JsonTarget {
        start: trimmed_start,
        end: trimmed_end,
    })
}

fn is_json_whitespace(byte: u8) -> bool {
    matches!(byte, b' ' | b'\n' | b'\r' | b'\t')
}

#[derive(Clone)]
struct JsonCandidate {
    start: usize,
    end: usize,
    priority: u8,
    path: Vec<JsonPathSegment>,
}

struct JsonSpanParser<'a> {
    source: &'a str,
    bytes: &'a [u8],
    position: usize,
    target: JsonTarget,
    nodes: usize,
    best: Option<JsonCandidate>,
}

impl<'a> JsonSpanParser<'a> {
    fn new(source: &'a str, target: JsonTarget) -> Self {
        Self {
            source,
            bytes: source.as_bytes(),
            position: 0,
            target,
            nodes: 0,
            best: None,
        }
    }

    fn parse(mut self) -> Result<JsonPath, JsonSelectionError> {
        self.skip_whitespace();
        let mut path = Vec::new();
        self.parse_value(&mut path)?;
        self.skip_whitespace();
        if self.position != self.bytes.len() {
            return self.invalid("unexpected content after the root value");
        }
        self.best
            .map(|candidate| JsonPath::new(candidate.path))
            .ok_or(JsonSelectionError::NoContainingValue)
    }

    fn parse_value(
        &mut self,
        path: &mut Vec<JsonPathSegment>,
    ) -> Result<(usize, usize), JsonSelectionError> {
        if path.len() > MAX_JSON_SELECTION_DEPTH {
            return Err(JsonSelectionError::DepthLimit {
                limit: MAX_JSON_SELECTION_DEPTH,
            });
        }
        self.nodes += 1;
        if self.nodes > MAX_JSON_SELECTION_NODES {
            return Err(JsonSelectionError::NodeLimit {
                limit: MAX_JSON_SELECTION_NODES,
            });
        }
        self.skip_whitespace();
        let start = self.position;
        match self.peek() {
            Some(b'{') => self.parse_object(path)?,
            Some(b'[') => self.parse_array(path)?,
            Some(b'"') => {
                self.parse_string()?;
            }
            Some(b't') => self.consume_literal(b"true")?,
            Some(b'f') => self.consume_literal(b"false")?,
            Some(b'n') => self.consume_literal(b"null")?,
            Some(b'-' | b'0'..=b'9') => self.parse_number()?,
            Some(_) => return self.invalid("expected a JSON value"),
            None => return self.invalid("expected a JSON value, found end of input"),
        }
        let end = self.position;
        self.consider(start, end, path, 1);
        Ok((start, end))
    }

    fn parse_object(&mut self, path: &mut Vec<JsonPathSegment>) -> Result<(), JsonSelectionError> {
        self.expect(b'{', "expected '{'")?;
        self.skip_whitespace();
        if self.consume_if(b'}') {
            return Ok(());
        }
        let mut keys = HashSet::new();
        loop {
            self.skip_whitespace();
            let key_start = self.position;
            if self.peek() != Some(b'"') {
                return self.invalid("expected a quoted object key");
            }
            let key = self.parse_string()?;
            let key_end = self.position;
            if !keys.insert(key.clone()) {
                return Err(JsonSelectionError::DuplicateObjectKey {
                    key,
                    offset: key_start,
                });
            }
            self.skip_whitespace();
            self.expect(b':', "expected ':' after object key")?;
            self.skip_whitespace();

            path.push(JsonPathSegment::Key(key));
            let (_, value_end) = self.parse_value(path)?;
            self.consider(key_start, key_end, path, 0);
            self.consider(key_start, value_end, path, 2);
            path.pop();

            self.skip_whitespace();
            if self.consume_if(b'}') {
                return Ok(());
            }
            self.expect(b',', "expected ',' or '}' after object member")?;
            self.skip_whitespace();
            if self.peek() == Some(b'}') {
                return self.invalid("trailing commas are not valid JSON");
            }
        }
    }

    fn parse_array(&mut self, path: &mut Vec<JsonPathSegment>) -> Result<(), JsonSelectionError> {
        self.expect(b'[', "expected '['")?;
        self.skip_whitespace();
        if self.consume_if(b']') {
            return Ok(());
        }
        let mut index = 0;
        loop {
            path.push(JsonPathSegment::Index(index));
            self.parse_value(path)?;
            path.pop();
            index += 1;

            self.skip_whitespace();
            if self.consume_if(b']') {
                return Ok(());
            }
            self.expect(b',', "expected ',' or ']' after array value")?;
            self.skip_whitespace();
            if self.peek() == Some(b']') {
                return self.invalid("trailing commas are not valid JSON");
            }
        }
    }

    fn parse_string(&mut self) -> Result<String, JsonSelectionError> {
        let start = self.position;
        self.expect(b'"', "expected a JSON string")?;
        while let Some(byte) = self.peek() {
            match byte {
                b'"' => {
                    self.position += 1;
                    return serde_json::from_str(&self.source[start..self.position]).map_err(
                        |error| JsonSelectionError::InvalidJson {
                            offset: start + error.column().saturating_sub(1),
                            message: error.to_string(),
                        },
                    );
                }
                b'\\' => {
                    self.position += 1;
                    if self.position >= self.bytes.len() {
                        return self.invalid("unterminated escape sequence in JSON string");
                    }
                    self.position += 1;
                }
                0x00..=0x1f => {
                    return self.invalid("unescaped control character in JSON string");
                }
                _ => self.position += 1,
            }
        }
        self.invalid("unterminated JSON string")
    }

    fn parse_number(&mut self) -> Result<(), JsonSelectionError> {
        let start = self.position;
        self.consume_if(b'-');
        match self.peek() {
            Some(b'0') => {
                self.position += 1;
                if matches!(self.peek(), Some(b'0'..=b'9')) {
                    return self.invalid("leading zero in JSON number");
                }
            }
            Some(b'1'..=b'9') => {
                self.position += 1;
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.position += 1;
                }
            }
            _ => return self.invalid("invalid JSON number"),
        }
        if self.consume_if(b'.') {
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return self.invalid("JSON number fraction requires a digit");
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.position += 1;
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.position += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.position += 1;
            }
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return self.invalid("JSON number exponent requires a digit");
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.position += 1;
            }
        }
        serde_json::from_str::<serde_json::Number>(&self.source[start..self.position]).map_err(
            |error| JsonSelectionError::InvalidJson {
                offset: start,
                message: error.to_string(),
            },
        )?;
        Ok(())
    }

    fn consume_literal(&mut self, expected: &[u8]) -> Result<(), JsonSelectionError> {
        let end = self.position.saturating_add(expected.len());
        if self.bytes.get(self.position..end) != Some(expected) {
            return self.invalid("invalid JSON literal");
        }
        self.position = end;
        Ok(())
    }

    fn consider(&mut self, start: usize, end: usize, path: &[JsonPathSegment], priority: u8) {
        let contains = if self.target.start == self.target.end {
            start <= self.target.start && self.target.start <= end
        } else {
            start <= self.target.start && self.target.end <= end
        };
        if !contains {
            return;
        }
        let candidate = JsonCandidate {
            start,
            end,
            priority,
            path: path.to_vec(),
        };
        let is_better = self.best.as_ref().is_none_or(|best| {
            let candidate_length = candidate.end.saturating_sub(candidate.start);
            let best_length = best.end.saturating_sub(best.start);
            candidate_length < best_length
                || (candidate_length == best_length && candidate.path.len() > best.path.len())
                || (candidate_length == best_length
                    && candidate.path.len() == best.path.len()
                    && candidate.priority < best.priority)
        });
        if is_better {
            self.best = Some(candidate);
        }
    }

    fn skip_whitespace(&mut self) {
        while self.peek().is_some_and(is_json_whitespace) {
            self.position += 1;
        }
    }

    fn expect(&mut self, expected: u8, message: &'static str) -> Result<(), JsonSelectionError> {
        if self.consume_if(expected) {
            Ok(())
        } else {
            self.invalid(message)
        }
    }

    fn consume_if(&mut self, expected: u8) -> bool {
        if self.peek() == Some(expected) {
            self.position += 1;
            true
        } else {
            false
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.position).copied()
    }

    fn invalid<T>(&self, message: impl Into<String>) -> Result<T, JsonSelectionError> {
        Err(JsonSelectionError::InvalidJson {
            offset: self.position,
            message: message.into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::thread;

    use super::*;
    use crate::core::request::ResponseHeader;

    fn request() -> RequestDraft {
        let mut request = RequestDraft::new("GET", "https://example.test/users");
        request
            .headers
            .push(super::super::HeaderEntry::new("Accept", "application/json"));
        request.body = r#"{"name":"Ada"}"#.to_owned();
        request
    }

    fn response() -> ResponseData {
        ResponseData {
            status: 200,
            status_text: "OK".to_owned(),
            http_version: "HTTP/2".to_owned(),
            final_url: "https://example.test/users".to_owned(),
            headers: vec![ResponseHeader {
                name: "content-type".to_owned(),
                value: "application/json".to_owned(),
            }],
            content_type: Some("application/json".to_owned()),
            body: br#"{"ok":true,"users":[{"name":"Ada"}]}"#.to_vec().into(),
            duration: Duration::from_millis(12),
        }
    }

    fn snippet(category: SnippetCategory, kind: SnippetKind, source: impl Into<String>) -> Snippet {
        let mut snippet = Snippet::new("Example", category, kind).unwrap();
        snippet.source = source.into();
        snippet
    }

    fn utf16_range_for(document: &str, selected: &str) -> SnippetTextRange {
        let start_byte = document.find(selected).expect("selected text must exist");
        let end_byte = start_byte + selected.len();
        SnippetTextRange::new(
            document[..start_byte].encode_utf16().count(),
            document[..end_byte].encode_utf16().count(),
        )
    }

    #[test]
    fn snippet_definition_normalizes_always_and_validates_metadata() {
        let mut value = Snippet::new(
            "  Log response  ",
            SnippetCategory::PostResponse,
            SnippetKind::Executable,
        )
        .unwrap();
        assert_eq!(value.name, "Log response");
        value.requirements = vec![SnippetRequirement::Always];
        value.normalize();
        assert!(value.requirements.is_empty());
        value.requirements = vec![
            SnippetRequirement::HasResponse,
            SnippetRequirement::HasResponse,
        ];
        assert!(matches!(
            value.validate(),
            Err(SnippetValidationError::DuplicateRequirement(
                SnippetRequirement::HasResponse
            ))
        ));
    }

    #[test]
    fn applicability_is_category_scoped_and_all_requirements_must_match() {
        let mut value = snippet(
            SnippetCategory::PostResponse,
            SnippetKind::Plain,
            "console.log(api.response.status);",
        );
        value.requirements = vec![
            SnippetRequirement::HasResponse,
            SnippetRequirement::HasSelection,
        ];
        let pre = SnippetInvocationContext::new(SnippetCategory::PreRequest, &request());
        let availability = value.availability(&pre);
        assert!(!availability.is_available());
        assert_eq!(availability.reasons().len(), 3);

        let selection = SnippetSelection::new(
            SnippetSelectionSource::PostResponse,
            SnippetSelectionArea::Script,
            "api.response",
            SnippetTextRange::new(0, 12),
            Some("application/javascript".to_owned()),
        )
        .unwrap();
        let post = SnippetInvocationContext::new(SnippetCategory::PostResponse, &request())
            .with_response(&response())
            .with_selection(selection);
        assert!(value.availability(&post).is_available());
    }

    #[test]
    fn plain_snippets_are_returned_without_parsing_javascript() {
        let value = snippet(
            SnippetCategory::PreRequest,
            SnippetKind::Plain,
            "this is intentionally not valid JavaScript }",
        );
        let generated = generate_snippet(
            &value,
            &SnippetInvocationContext::new(SnippetCategory::PreRequest, &request()),
            &SnippetCancellation::new(),
        )
        .unwrap();
        assert_eq!(generated.text, value.source);
        assert_eq!(generated.report.duration, Duration::ZERO);
    }

    #[test]
    fn json_selection_maps_key_member_and_nested_value_by_source_span() {
        let document = r#"{
  "users": [
    {"display name": "Ada"},
    {"display name": "😀"}
  ]
}"#;
        let range = utf16_range_for(document, r#""display name": "😀""#);
        let selection = SnippetSelection::from_json_document(
            SnippetSelectionSource::Response,
            SnippetSelectionArea::Body,
            document,
            range,
            Some("application/json".to_owned()),
        )
        .unwrap();
        let path = selection.json_path().unwrap();
        assert_eq!(
            path.segments(),
            &[
                JsonPathSegment::Key("users".to_owned()),
                JsonPathSegment::Index(1),
                JsonPathSegment::Key("display name".to_owned()),
            ]
        );
        assert_eq!(path.json_pointer(), "/users/1/display name");
        assert_eq!(path.json_path(), r#"$.users[1]["display name"]"#);
        assert_eq!(
            path.expression("api.response.json()").unwrap(),
            r#"api.response.json().users[1]["display name"]"#
        );
    }

    #[test]
    fn json_selection_uses_utf16_offsets_and_rejects_surrogate_splits() {
        let document = r#"{"emoji":"😀","next":true}"#;
        let range = utf16_range_for(document, "😀");
        let selection = SnippetSelection::from_json_document(
            SnippetSelectionSource::Response,
            SnippetSelectionArea::Body,
            document,
            range,
            None,
        )
        .unwrap();
        assert_eq!(
            selection.json_path().unwrap().segments(),
            &[JsonPathSegment::Key("emoji".to_owned())]
        );
        assert!(matches!(
            SnippetSelection::from_json_document(
                SnippetSelectionSource::Response,
                SnippetSelectionArea::Body,
                document,
                SnippetTextRange::new(range.start + 1, range.start + 1),
                None,
            ),
            Err(JsonSelectionError::InvalidUtf16Offset { .. })
        ));
    }

    #[test]
    fn json_selection_rejects_invalid_json_and_whitespace_only_ranges() {
        let invalid = r#"{"a": 1,}"#;
        assert!(matches!(
            SnippetSelection::from_json_document(
                SnippetSelectionSource::Response,
                SnippetSelectionArea::Body,
                invalid,
                SnippetTextRange::new(1, 2),
                None,
            ),
            Err(JsonSelectionError::InvalidJson { .. })
        ));
        let document = "{ \n \"a\": 1 }";
        assert_eq!(
            SnippetSelection::from_json_document(
                SnippetSelectionSource::Response,
                SnippetSelectionArea::Body,
                document,
                SnippetTextRange::new(1, 3),
                None,
            )
            .unwrap_err(),
            JsonSelectionError::WhitespaceOnly
        );

        let duplicates = r#"{"token":"first","token":"second"}"#;
        assert!(matches!(
            SnippetSelection::from_json_document(
                SnippetSelectionSource::Response,
                SnippetSelectionArea::Body,
                duplicates,
                utf16_range_for(duplicates, r#""second""#),
                None,
            ),
            Err(JsonSelectionError::DuplicateObjectKey { key, .. }) if key == "token"
        ));
    }

    #[test]
    fn executable_snippet_exposes_only_the_frozen_phase_specific_api_snapshot() {
        let value = snippet(
            SnippetCategory::PostResponse,
            SnippetKind::Executable,
            r#"
return snippet.result(
  `${api.request.method}:${api.response.status}:${api.response.contentType}:${api.response.json().ok}:${Object.isFrozen(api.request)}:${"request" in snippet}:${"response" in snippet}`,
  { cursor: 3 },
);
"#,
        );
        let context = SnippetInvocationContext::new(SnippetCategory::PostResponse, &request())
            .with_response(&response());
        let generated = generate_snippet(&value, &context, &SnippetCancellation::new()).unwrap();
        assert_eq!(
            generated.text,
            "GET:200:application/json:true:true:false:false"
        );
        assert_eq!(generated.cursor, Some(3));
    }

    #[test]
    fn pre_request_generator_has_no_response_property_and_cannot_mutate_request() {
        let visibility = snippet(
            SnippetCategory::PreRequest,
            SnippetKind::Executable,
            r#"return `${"response" in api}:${"request" in snippet}:${"response" in snippet}:${typeof fetch}`;"#,
        );
        let context = SnippetInvocationContext::new(SnippetCategory::PreRequest, &request())
            .with_response(&response());
        let generated =
            generate_snippet(&visibility, &context, &SnippetCancellation::new()).unwrap();
        assert_eq!(generated.text, "false:false:false:undefined");

        let mutation = snippet(
            SnippetCategory::PreRequest,
            SnippetKind::Executable,
            r#"api.request.method = "DELETE"; return api.request.method;"#,
        );
        let error = generate_snippet(&mutation, &context, &SnippetCancellation::new()).unwrap_err();
        assert_eq!(error.diagnostic.kind, SnippetErrorKind::Runtime);
        assert!(
            error
                .diagnostic
                .message
                .to_ascii_lowercase()
                .contains("read")
        );
    }

    #[test]
    fn write_and_selection_helpers_generate_code_without_host_side_js_parsing() {
        let document = r#"{"users":[{"name":"Ada"}]}"#;
        let selection = SnippetSelection::from_json_document(
            SnippetSelectionSource::Response,
            SnippetSelectionArea::Body,
            document,
            utf16_range_for(document, r#""Ada""#),
            Some("application/json".to_owned()),
        )
        .unwrap();
        let value = snippet(
            SnippetCategory::PostResponse,
            SnippetKind::Executable,
            r#"
console.info(snippet.selection.jsonPointer());
write(`console.log(${snippet.selection.expression()});`);
"#,
        );
        let context = SnippetInvocationContext::new(SnippetCategory::PostResponse, &request())
            .with_response(&response())
            .with_selection(selection);
        let generated = generate_snippet(&value, &context, &SnippetCancellation::new()).unwrap();
        assert_eq!(
            generated.text,
            "console.log(api.response.json().users[0].name);"
        );
        assert_eq!(generated.report.logs[0].message, "/users/0/name");
    }

    #[test]
    fn response_selection_expression_requires_an_explicit_root_in_pre_request_generators() {
        let document = r#"{"users":[{"name":"Ada"}]}"#;
        let selection = SnippetSelection::from_json_document(
            SnippetSelectionSource::Response,
            SnippetSelectionArea::Body,
            document,
            utf16_range_for(document, r#""Ada""#),
            Some("application/json".to_owned()),
        )
        .unwrap();
        let context = SnippetInvocationContext::new(SnippetCategory::PreRequest, &request())
            .with_selection(selection);
        let inferred = snippet(
            SnippetCategory::PreRequest,
            SnippetKind::Executable,
            "return snippet.selection.expression();",
        );
        let error = generate_snippet(&inferred, &context, &SnippetCancellation::new()).unwrap_err();
        assert_eq!(error.diagnostic.kind, SnippetErrorKind::Runtime);
        assert!(error.diagnostic.message.contains("root expression"));

        let explicit = snippet(
            SnippetCategory::PreRequest,
            SnippetKind::Executable,
            r#"return snippet.selection.expression({ root: "payload" });"#,
        );
        assert_eq!(
            generate_snippet(&explicit, &context, &SnippetCancellation::new())
                .unwrap()
                .text,
            "payload.users[0].name"
        );
    }

    #[test]
    fn generator_rejects_mixed_write_and_return_and_invalid_cursor() {
        let context = SnippetInvocationContext::new(SnippetCategory::PreRequest, &request());
        let mixed = snippet(
            SnippetCategory::PreRequest,
            SnippetKind::Executable,
            r#"write("first"); return "second";"#,
        );
        let error = generate_snippet(&mixed, &context, &SnippetCancellation::new()).unwrap_err();
        assert_eq!(error.diagnostic.kind, SnippetErrorKind::InvalidOutput);

        let cursor = snippet(
            SnippetCategory::PreRequest,
            SnippetKind::Executable,
            r#"return snippet.result("😀", { cursor: 3 });"#,
        );
        let error = generate_snippet(&cursor, &context, &SnippetCancellation::new()).unwrap_err();
        assert_eq!(error.diagnostic.kind, SnippetErrorKind::InvalidOutput);

        let split_surrogate = snippet(
            SnippetCategory::PreRequest,
            SnippetKind::Executable,
            r#"return snippet.result("😀", { cursor: 1 });"#,
        );
        let error =
            generate_snippet(&split_surrogate, &context, &SnippetCancellation::new()).unwrap_err();
        assert_eq!(error.diagnostic.kind, SnippetErrorKind::InvalidOutput);
        assert!(error.diagnostic.message.contains("surrogate pair"));

        let valid_surrogate_boundary = snippet(
            SnippetCategory::PreRequest,
            SnippetKind::Executable,
            r#"return snippet.result("😀", { cursor: 2 });"#,
        );
        assert_eq!(
            generate_snippet(
                &valid_surrogate_boundary,
                &context,
                &SnippetCancellation::new()
            )
            .unwrap()
            .cursor,
            Some(2)
        );
    }

    #[test]
    fn generator_limits_output_and_rejects_async_results() {
        let context = SnippetInvocationContext::new(SnippetCategory::PreRequest, &request());
        let oversized = snippet(
            SnippetCategory::PreRequest,
            SnippetKind::Executable,
            format!("return 'x'.repeat({});", MAX_SNIPPET_OUTPUT_BYTES + 1),
        );
        let error =
            generate_snippet(&oversized, &context, &SnippetCancellation::new()).unwrap_err();
        assert_eq!(error.diagnostic.kind, SnippetErrorKind::OutputLimit);

        let asynchronous = snippet(
            SnippetCategory::PreRequest,
            SnippetKind::Executable,
            "return Promise.resolve('later');",
        );
        let error =
            generate_snippet(&asynchronous, &context, &SnippetCancellation::new()).unwrap_err();
        assert_eq!(error.diagnostic.kind, SnippetErrorKind::InvalidOutput);
        assert!(error.diagnostic.message.contains("async"));
    }

    #[test]
    fn generator_return_contract_accepts_structural_results_and_rejects_numbers() {
        let context = SnippetInvocationContext::new(SnippetCategory::PreRequest, &request());
        let numeric = snippet(
            SnippetCategory::PreRequest,
            SnippetKind::Executable,
            "return 42;",
        );
        let error = generate_snippet(&numeric, &context, &SnippetCancellation::new()).unwrap_err();
        assert_eq!(error.diagnostic.kind, SnippetErrorKind::InvalidOutput);

        let structural = snippet(
            SnippetCategory::PreRequest,
            SnippetKind::Executable,
            "return { text: 'structural result' };",
        );
        let generated =
            generate_snippet(&structural, &context, &SnippetCancellation::new()).unwrap();
        assert_eq!(generated.text, "structural result");
        assert_eq!(generated.cursor, None);
    }

    #[test]
    fn runtime_errors_keep_bounded_redacted_logs() {
        let value = snippet(
            SnippetCategory::PreRequest,
            SnippetKind::Executable,
            r#"
console.info("before", "very-secret");
throw new Error("very-secret");
"#,
        );
        let context = SnippetInvocationContext::new(SnippetCategory::PreRequest, &request())
            .with_sensitive_values(["very-secret".to_owned()]);
        let error = generate_snippet(&value, &context, &SnippetCancellation::new()).unwrap_err();
        assert_eq!(error.diagnostic.message, "[REDACTED]");
        assert_eq!(error.report.logs[0].message, "before [REDACTED]");
        assert!(
            error
                .diagnostic
                .stack
                .as_deref()
                .is_some_and(|stack| stack.contains("pre-request-snippet.js"))
        );
    }

    #[test]
    fn log_redaction_captures_across_the_host_truncation_boundary() {
        let value = snippet(
            SnippetCategory::PreRequest,
            SnippetKind::Executable,
            format!(
                r#"console.log("x".repeat({}) + "LEAKME"); throw new Error("stop");"#,
                MAX_SNIPPET_LOG_BYTES - 3
            ),
        );
        let context = SnippetInvocationContext::new(SnippetCategory::PreRequest, &request())
            .with_sensitive_values(["LEAKME".to_owned()]);
        let error = generate_snippet(&value, &context, &SnippetCancellation::new()).unwrap_err();
        let message = &error.report.logs[0].message;
        assert!(message.len() <= MAX_SNIPPET_LOG_BYTES);
        assert!(!message.contains("LEAKME"));
        assert!(!message.contains("LEA"));
    }

    #[test]
    fn internal_result_collectors_cannot_inject_or_mutate_private_logs() {
        let value = snippet(
            SnippetCategory::PreRequest,
            SnippetKind::Executable,
            r#"
try { __RESOLVED_SNIPPET_PARTIAL().logs.push({ level: "error", message: "injected" }); } catch (_) {}
Array.prototype.map = function () { return this; };
Object.freeze = value => value;
Reflect.apply = () => [];
try { __RESOLVED_SNIPPET_FINISH("ignored").logs[0] = { level: "error", message: "injected" }; } catch (_) {}
try { __RESOLVED_SNIPPET_PARTIAL().logs.push({ level: "error", message: "injected" }); } catch (_) {}
return "ok";
"#,
        );
        let context = SnippetInvocationContext::new(SnippetCategory::PreRequest, &request());
        let generated = generate_snippet(&value, &context, &SnippetCancellation::new()).unwrap();
        assert_eq!(generated.text, "ok");
        assert!(generated.report.logs.is_empty());
    }

    #[test]
    fn diagnostics_are_redacted_then_utf8_safely_bounded() {
        let value = snippet(
            SnippetCategory::PreRequest,
            SnippetKind::Executable,
            format!(
                r#"
const error = new Error("😀" + "x".repeat({}) + "LEAKME");
error.stack = "😀" + "x".repeat({}) + "LEAKME";
throw error;
"#,
                MAX_SNIPPET_DIAGNOSTIC_MESSAGE_BYTES - 7,
                MAX_SNIPPET_DIAGNOSTIC_STACK_BYTES - 7,
            ),
        );
        let context = SnippetInvocationContext::new(SnippetCategory::PreRequest, &request())
            .with_sensitive_values(["LEAKME".to_owned()]);
        let error = generate_snippet(&value, &context, &SnippetCancellation::new()).unwrap_err();
        assert_eq!(error.diagnostic.kind, SnippetErrorKind::Runtime);
        assert!(error.diagnostic.message.len() <= MAX_SNIPPET_DIAGNOSTIC_MESSAGE_BYTES);
        assert!(!error.diagnostic.message.contains("LEAKME"));
        let stack = error.diagnostic.stack.unwrap();
        assert!(stack.len() <= MAX_SNIPPET_DIAGNOSTIC_STACK_BYTES);
        assert!(!stack.contains("LEAKME"));
        assert!(std::str::from_utf8(stack.as_bytes()).is_ok());
    }

    #[test]
    fn context_copy_and_serialization_limits_reject_before_engine_allocation() {
        let executable = snippet(
            SnippetCategory::PreRequest,
            SnippetKind::Executable,
            "return 'ok';",
        );

        let mut oversized = request();
        oversized.url = "x".repeat(MAX_SNIPPET_CONTEXT_BYTES + 1);
        let context = SnippetInvocationContext::new(SnippetCategory::PreRequest, &oversized);
        let error =
            generate_snippet(&executable, &context, &SnippetCancellation::new()).unwrap_err();
        assert_eq!(error.diagnostic.kind, SnippetErrorKind::ContextLimit);

        let mut escaped = request();
        escaped.headers.push(super::super::HeaderEntry::new(
            "Escaped",
            "\"".repeat(MAX_SNIPPET_CONTEXT_BYTES / 2),
        ));
        let context = SnippetInvocationContext::new(SnippetCategory::PreRequest, &escaped);
        let error =
            generate_snippet(&executable, &context, &SnippetCancellation::new()).unwrap_err();
        assert_eq!(error.diagnostic.kind, SnippetErrorKind::ContextLimit);
        assert!(error.diagnostic.message.contains("serialized limit"));
    }

    #[test]
    fn context_row_and_sensitive_value_budgets_are_shared_and_reject_only() {
        let mut exact = request();
        exact.headers = (0..MAX_SNIPPET_CONTEXT_ROWS - 1)
            .map(|_| super::super::HeaderEntry::new("", ""))
            .collect();
        exact.body_fields.push(super::super::BodyField::default());
        let context = SnippetInvocationContext::new(SnippetCategory::PostResponse, &exact);
        assert!(context.resource_error().is_none());
        let context = context.with_response(&response());
        assert!(context.resource_error().is_some());

        let mut request_with_large_header = request();
        request_with_large_header
            .headers
            .push(super::super::HeaderEntry::new(
                "Large",
                "x".repeat(6 * 1024 * 1024),
            ));
        let context =
            SnippetInvocationContext::new(SnippetCategory::PreRequest, &request_with_large_header)
                .with_sensitive_values(["s".repeat(3 * 1024 * 1024)]);
        assert!(context.resource_error().is_some());
        let executable = snippet(
            SnippetCategory::PreRequest,
            SnippetKind::Executable,
            "return 'ok';",
        );
        let error =
            generate_snippet(&executable, &context, &SnippetCancellation::new()).unwrap_err();
        assert_eq!(error.diagnostic.kind, SnippetErrorKind::ContextLimit);
    }

    #[test]
    fn capped_context_writer_accepts_the_limit_and_rejects_the_next_byte() {
        let mut writer = ContextSizeWriter {
            bytes: MAX_SNIPPET_CONTEXT_BYTES - 2,
            exceeded: false,
        };
        writer.write_all(&[1, 2]).unwrap();
        assert_eq!(writer.bytes, MAX_SNIPPET_CONTEXT_BYTES);
        assert!(writer.write_all(&[3]).is_err());
        assert!(writer.exceeded);
        assert_eq!(writer.bytes, MAX_SNIPPET_CONTEXT_BYTES);
    }

    #[test]
    fn interruption_checkpoints_prefer_cancellation_and_cover_finish() {
        let cancellation = SnippetCancellation::new();
        cancellation.cancel();
        let timed_out = AtomicBool::new(false);
        assert_eq!(
            interruption_kind(
                &cancellation,
                &timed_out,
                Instant::now() - Duration::from_millis(1)
            ),
            Some(SnippetErrorKind::Cancelled)
        );
        let active = SnippetCancellation::new();
        assert_eq!(
            interruption_kind(
                &active,
                &timed_out,
                Instant::now() - Duration::from_millis(1)
            ),
            Some(SnippetErrorKind::TimedOut)
        );

        let context = SnippetInvocationContext::new(SnippetCategory::PreRequest, &request());
        let looping_getter = snippet(
            SnippetCategory::PreRequest,
            SnippetKind::Executable,
            "return { get then() { while (true) {} } };",
        );
        let cancellation = SnippetCancellation::new();
        let worker_cancellation = cancellation.clone();
        let worker = thread::spawn(move || {
            generate_snippet(&looping_getter, &context, &worker_cancellation)
        });
        thread::sleep(Duration::from_millis(20));
        cancellation.cancel();
        let error = worker.join().unwrap().unwrap_err();
        assert_eq!(error.diagnostic.kind, SnippetErrorKind::Cancelled);

        let timeout = snippet(
            SnippetCategory::PreRequest,
            SnippetKind::Executable,
            "return { get then() { while (true) {} } };",
        );
        let context = SnippetInvocationContext::new(SnippetCategory::PreRequest, &request());
        let error = generate_snippet(&timeout, &context, &SnippetCancellation::new()).unwrap_err();
        assert_eq!(error.diagnostic.kind, SnippetErrorKind::TimedOut);
    }

    #[test]
    fn executable_runtimes_are_fresh_and_cancellable() {
        let context = SnippetInvocationContext::new(SnippetCategory::PreRequest, &request());
        let leak = snippet(
            SnippetCategory::PreRequest,
            SnippetKind::Executable,
            "Array.prototype.leaked = true; return 'ok';",
        );
        generate_snippet(&leak, &context, &SnippetCancellation::new()).unwrap();
        let check = snippet(
            SnippetCategory::PreRequest,
            SnippetKind::Executable,
            "return String(Boolean(Array.prototype.leaked));",
        );
        assert_eq!(
            generate_snippet(&check, &context, &SnippetCancellation::new())
                .unwrap()
                .text,
            "false"
        );

        let cancellation = SnippetCancellation::new();
        let worker_cancellation = cancellation.clone();
        let looping = snippet(
            SnippetCategory::PreRequest,
            SnippetKind::Executable,
            "while (true) {}",
        );
        let worker_context = context.clone();
        let worker = thread::spawn(move || {
            generate_snippet(&looping, &worker_context, &worker_cancellation)
        });
        thread::sleep(Duration::from_millis(20));
        cancellation.cancel();
        let error = worker.join().unwrap().unwrap_err();
        assert_eq!(error.diagnostic.kind, SnippetErrorKind::Cancelled);
    }
}

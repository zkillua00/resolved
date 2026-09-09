use std::collections::{HashMap, HashSet};
use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::request::{BodyMode, RequestDraft};
use super::websocket::WebSocketWorkspace;
use super::workspace::{Environment, RequestScripts};

const MAX_VARIABLE_DEPTH: usize = 32;
pub(crate) const REDACTED_VALUE: &str = "[REDACTED]";

/// A persistable request definition.
///
/// The request remains an unexpanded template. Variable expansion produces a
/// separate [`ResolvedRequest`] immediately before the network request starts.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RequestTemplate {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub documentation: String,
    #[serde(default)]
    pub request: RequestDraft,
    #[serde(default)]
    pub scripts: RequestScripts,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub websocket: Option<WebSocketWorkspace>,
}

impl RequestTemplate {
    pub fn new(request: RequestDraft) -> Self {
        Self {
            request,
            scripts: RequestScripts::default(),
            documentation: String::new(),
            websocket: None,
        }
    }

    pub fn websocket(document: WebSocketWorkspace) -> Self {
        Self {
            websocket: Some(document),
            ..Self::default()
        }
    }

    pub fn is_websocket(&self) -> bool {
        self.websocket.is_some()
    }

    #[allow(dead_code)]
    pub fn with_scripts(mut self, scripts: RequestScripts) -> Self {
        self.scripts = scripts;
        self
    }

    #[allow(dead_code)]
    pub fn resolve(
        &self,
        environment: Option<&Environment>,
    ) -> Result<ResolvedRequest, VariableResolutionError> {
        resolve_request(&self.request, environment)
    }
}

impl From<RequestDraft> for RequestTemplate {
    fn from(request: RequestDraft) -> Self {
        Self::new(request)
    }
}

/// Identifies the request field in which variable expansion failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TemplateField {
    Url,
    QueryParamKey(usize),
    QueryParamValue(usize),
    HeaderName(usize),
    HeaderValue(usize),
    Body,
    BodyFieldName(usize),
    BodyFieldValue(usize),
}

impl fmt::Display for TemplateField {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Url => formatter.write_str("URL"),
            Self::QueryParamKey(index) => {
                write!(formatter, "query parameter {} key", index + 1)
            }
            Self::QueryParamValue(index) => {
                write!(formatter, "query parameter {} value", index + 1)
            }
            Self::HeaderName(index) => write!(formatter, "header {} name", index + 1),
            Self::HeaderValue(index) => write!(formatter, "header {} value", index + 1),
            Self::Body => formatter.write_str("body"),
            Self::BodyFieldName(index) => write!(formatter, "body field {} name", index + 1),
            Self::BodyFieldValue(index) => write!(formatter, "body field {} value", index + 1),
        }
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum VariableResolutionError {
    #[error("environment has more than one enabled variable named '{name}'")]
    DuplicateVariable { name: String },

    #[error("variable '{name}' used in {field} is disabled")]
    DisabledVariable { field: TemplateField, name: String },

    #[error("variable '{name}' used in {field} is not defined")]
    UndefinedVariable { field: TemplateField, name: String },

    #[error("empty variable placeholder in {field} at byte {offset}")]
    EmptyPlaceholder { field: TemplateField, offset: usize },

    #[error("unclosed variable placeholder in {field} at byte {offset}")]
    UnclosedPlaceholder { field: TemplateField, offset: usize },

    #[error("invalid variable name '{name}' in {field}")]
    InvalidVariableName { field: TemplateField, name: String },

    #[error("variable cycle in {field}: {chain}")]
    VariableCycle { field: TemplateField, chain: String },

    #[error("variable expansion exceeded {limit} levels in {field}")]
    DepthExceeded { field: TemplateField, limit: usize },
}

/// The ephemeral request sent to the network layer.
///
/// `sensitive_values` contains only non-empty, resolved values of secret
/// variables actually used by this request. It can be used to scrub errors,
/// logs, and the displayed final URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedRequest {
    pub request: RequestDraft,
    pub used_variables: Vec<String>,
    pub sensitive_values: Vec<String>,
}

impl ResolvedRequest {
    pub fn redact_secrets(&self, text: &str) -> String {
        redact_secret_values(text, &self.sensitive_values)
    }
}

pub(crate) fn redact_secret_values(text: &str, sensitive_values: &[String]) -> String {
    secret_variants(sensitive_values)
        .into_iter()
        .fold(text.to_owned(), |text, value| {
            text.replace(&value, REDACTED_VALUE)
        })
}

pub(crate) fn redact_secret_bytes(bytes: &[u8], sensitive_values: &[String]) -> Vec<u8> {
    secret_variants(sensitive_values)
        .into_iter()
        .fold(bytes.to_vec(), |bytes, value| {
            replace_bytes(&bytes, value.as_bytes(), REDACTED_VALUE.as_bytes())
        })
}

/// Derives the distinct spellings under which a secret can appear (raw,
/// percent-encoded, URL-path-escaped, query-escaped), longest-first, deduped.
///
/// Callers that scrub repeatedly should derive once and cache the result
/// (`SecretRedactor` does) instead of re-deriving on every call.
pub(crate) fn secret_variants(sensitive_values: &[String]) -> Vec<String> {
    let values = sensitive_values
        .iter()
        .filter(|value| !value.is_empty())
        .cloned()
        .collect::<Vec<_>>();
    let mut variants = values
        .iter()
        .flat_map(|value| secret_spellings(value))
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    variants.sort_by_key(|value| std::cmp::Reverse(value.len()));
    variants.dedup();
    variants
}

fn replace_bytes(bytes: &[u8], needle: &[u8], replacement: &[u8]) -> Vec<u8> {
    if needle.is_empty() {
        return bytes.to_vec();
    }
    let mut output = Vec::with_capacity(bytes.len());
    let mut cursor = 0;
    for position in memchr::memmem::find_iter(bytes, needle) {
        output.extend_from_slice(&bytes[cursor..position]);
        output.extend_from_slice(replacement);
        cursor = position + needle.len();
    }
    output.extend_from_slice(&bytes[cursor..]);
    output
}

fn secret_spellings(value: &str) -> Vec<String> {
    let mut spellings = vec![value.to_owned()];
    spellings.push(url::form_urlencoded::byte_serialize(value.as_bytes()).collect());

    let mut path_url = url::Url::parse("https://redaction.invalid/").expect("static URL is valid");
    if let Ok(mut segments) = path_url.path_segments_mut() {
        segments.push(value);
    }
    spellings.push(path_url.path().trim_start_matches('/').to_owned());

    if let Ok(parsed) = url::Url::parse(&format!("https://redaction.invalid/{value}")) {
        spellings.push(parsed.path().trim_start_matches('/').to_owned());
    }
    if let Ok(parsed) = url::Url::parse(&format!("https://redaction.invalid/?value={value}"))
        && let Some(query) = parsed.query()
        && let Some(encoded) = query.strip_prefix("value=")
    {
        spellings.push(encoded.to_owned());
    }

    spellings
}

/// Resolve all enabled outgoing request fields against one environment.
///
/// Disabled header and structured-body rows are deliberately left untouched
/// because the request layer does not send them. Inactive body representations
/// are also preserved without expansion, so stale hidden editor contents cannot
/// block a request that will not put them on the wire.
pub fn resolve_request(
    template: &RequestDraft,
    environment: Option<&Environment>,
) -> Result<ResolvedRequest, VariableResolutionError> {
    let mut resolver = Resolver::new(environment)?;
    let mut request = template.clone();

    request.url = resolver.resolve_text(&request.url, TemplateField::Url)?;
    for (index, param) in request.query_params.iter_mut().enumerate() {
        if !param.enabled {
            continue;
        }
        param.key = resolver.resolve_text(&param.key, TemplateField::QueryParamKey(index))?;
        param.value = resolver.resolve_text(&param.value, TemplateField::QueryParamValue(index))?;
    }
    for (index, header) in request.headers.iter_mut().enumerate() {
        if !header.enabled {
            continue;
        }
        header.name = resolver.resolve_text(&header.name, TemplateField::HeaderName(index))?;
        header.value = resolver.resolve_text(&header.value, TemplateField::HeaderValue(index))?;
    }
    match request.body_mode {
        BodyMode::None => {}
        BodyMode::Raw => {
            request.body = resolver.resolve_text(&request.body, TemplateField::Body)?;
        }
        BodyMode::FormUrlEncoded | BodyMode::MultipartFormData => {
            for (index, field) in request.body_fields.iter_mut().enumerate() {
                if !field.enabled {
                    continue;
                }
                field.name =
                    resolver.resolve_text(&field.name, TemplateField::BodyFieldName(index))?;
                field.value =
                    resolver.resolve_text(&field.value, TemplateField::BodyFieldValue(index))?;
            }
        }
    }

    Ok(ResolvedRequest {
        request,
        used_variables: resolver.used_variables,
        sensitive_values: resolver.sensitive_values,
    })
}

struct Resolver<'a> {
    enabled: HashMap<String, &'a super::workspace::EnvironmentVariable>,
    disabled: HashSet<String>,
    used_variables: Vec<String>,
    used_set: HashSet<String>,
    sensitive_values: Vec<String>,
    sensitive_set: HashSet<String>,
}

impl<'a> Resolver<'a> {
    fn new(environment: Option<&'a Environment>) -> Result<Self, VariableResolutionError> {
        let mut enabled = HashMap::new();
        let mut disabled = HashSet::new();

        if let Some(environment) = environment {
            for variable in &environment.variables {
                let name = variable.key.trim().to_owned();
                if variable.enabled {
                    if enabled.insert(name.clone(), variable).is_some() {
                        return Err(VariableResolutionError::DuplicateVariable { name });
                    }
                    disabled.remove(&name);
                } else if !enabled.contains_key(&name) {
                    disabled.insert(name);
                }
            }
        }

        Ok(Self {
            enabled,
            disabled,
            used_variables: Vec::new(),
            used_set: HashSet::new(),
            sensitive_values: Vec::new(),
            sensitive_set: HashSet::new(),
        })
    }

    fn resolve_text(
        &mut self,
        text: &str,
        field: TemplateField,
    ) -> Result<String, VariableResolutionError> {
        self.resolve_text_inner(text, &field, &mut Vec::new(), 0)
    }

    fn resolve_text_inner(
        &mut self,
        text: &str,
        field: &TemplateField,
        stack: &mut Vec<String>,
        depth: usize,
    ) -> Result<String, VariableResolutionError> {
        if depth > MAX_VARIABLE_DEPTH {
            return Err(VariableResolutionError::DepthExceeded {
                field: field.clone(),
                limit: MAX_VARIABLE_DEPTH,
            });
        }

        let mut output = String::with_capacity(text.len());
        let mut cursor = 0;

        while cursor < text.len() {
            let opening = text[cursor..].find("{{").map(|offset| cursor + offset);

            let Some(opening) = opening else {
                output.push_str(&text[cursor..]);
                break;
            };

            output.push_str(&text[cursor..opening]);
            let name_start = opening + 2;
            let Some(relative_closing) = text[name_start..].find("}}") else {
                return Err(VariableResolutionError::UnclosedPlaceholder {
                    field: field.clone(),
                    offset: opening,
                });
            };
            let closing = name_start + relative_closing;
            let name = text[name_start..closing].trim();

            if name.is_empty() {
                return Err(VariableResolutionError::EmptyPlaceholder {
                    field: field.clone(),
                    offset: opening,
                });
            }
            if name.contains("{{") || name.contains("}}") {
                return Err(VariableResolutionError::InvalidVariableName {
                    field: field.clone(),
                    name: name.to_owned(),
                });
            }

            let value = self.resolve_variable(name, field, stack, depth + 1)?;
            output.push_str(&value);
            cursor = closing + 2;
        }

        Ok(output)
    }

    fn resolve_variable(
        &mut self,
        name: &str,
        field: &TemplateField,
        stack: &mut Vec<String>,
        depth: usize,
    ) -> Result<String, VariableResolutionError> {
        if depth > MAX_VARIABLE_DEPTH {
            return Err(VariableResolutionError::DepthExceeded {
                field: field.clone(),
                limit: MAX_VARIABLE_DEPTH,
            });
        }
        if let Some(cycle_start) = stack.iter().position(|entry| entry == name) {
            let mut cycle = stack[cycle_start..].to_vec();
            cycle.push(name.to_owned());
            return Err(VariableResolutionError::VariableCycle {
                field: field.clone(),
                chain: cycle.join(" -> "),
            });
        }

        let Some(variable) = self.enabled.get(name).copied() else {
            return Err(if self.disabled.contains(name) {
                VariableResolutionError::DisabledVariable {
                    field: field.clone(),
                    name: name.to_owned(),
                }
            } else {
                VariableResolutionError::UndefinedVariable {
                    field: field.clone(),
                    name: name.to_owned(),
                }
            });
        };

        if self.used_set.insert(name.to_owned()) {
            self.used_variables.push(name.to_owned());
        }

        stack.push(name.to_owned());
        let value = self.resolve_text_inner(&variable.value, field, stack, depth)?;
        stack.pop();

        if variable.secret && !value.is_empty() && self.sensitive_set.insert(value.clone()) {
            self.sensitive_values.push(value.clone());
        }

        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::request::{BodyField, BodyFieldKind, BodyMode, HeaderEntry, QueryParamEntry};
    use crate::core::workspace::EnvironmentVariable;

    fn environment(variables: Vec<EnvironmentVariable>) -> Environment {
        Environment {
            id: "environment-test".to_owned(),
            name: "Test".to_owned(),
            created_by: None,
            variables,
        }
    }

    fn variable(key: &str, value: &str, enabled: bool, secret: bool) -> EnvironmentVariable {
        EnvironmentVariable {
            id: format!("variable-{key}"),
            key: key.to_owned(),
            value: value.to_owned(),
            enabled,
            secret,
            created_by: None,
        }
    }

    #[test]
    fn resolves_url_headers_and_body_without_mutating_template() {
        let environment = environment(vec![
            variable("base_url", "https://example.com", true, false),
            variable("token", "top-secret", true, true),
            variable("item_id", "42", true, false),
        ]);
        let template = RequestDraft {
            method: "POST".to_owned(),
            url: "{{ base_url }}/items/{{item_id}}".to_owned(),
            headers: vec![HeaderEntry::new("Authorization", "Bearer {{token}}")],
            body: r#"{"item":"{{ item_id }}"}"#.to_owned(),
            ..RequestDraft::default()
        };

        let resolved = resolve_request(&template, Some(&environment)).unwrap();

        assert_eq!(resolved.request.url, "https://example.com/items/42");
        assert_eq!(resolved.request.headers[0].value, "Bearer top-secret");
        assert_eq!(resolved.request.body, r#"{"item":"42"}"#);
        assert_eq!(
            template.url, "{{ base_url }}/items/{{item_id}}",
            "resolution must not mutate the persisted template"
        );
        assert_eq!(resolved.sensitive_values, vec!["top-secret"]);
        assert_eq!(
            resolved.redact_secrets("failed for Bearer top-secret"),
            "failed for Bearer [REDACTED]"
        );
    }

    #[test]
    fn resolves_enabled_structured_body_fields_and_tracks_secrets() {
        let environment = environment(vec![
            variable("key", "username", true, false),
            variable("value", "Ada Lovelace", true, false),
            variable("file", "/tmp/{{token}}.txt", true, false),
            variable("token", "private-token", true, true),
        ]);
        let mut template = RequestDraft::new("POST", "https://example.com/upload");
        template.body_mode = BodyMode::MultipartFormData;
        template.body_fields = vec![
            BodyField::text("{{key}}", "{{value}}"),
            BodyField {
                enabled: true,
                name: "attachment".to_owned(),
                value: "{{file}}".to_owned(),
                kind: BodyFieldKind::File,
            },
            BodyField {
                enabled: false,
                name: "{{missing}}".to_owned(),
                value: "{{missing}}".to_owned(),
                kind: BodyFieldKind::Text,
            },
        ];

        let resolved = resolve_request(&template, Some(&environment)).unwrap();

        assert_eq!(resolved.request.body_fields[0].name, "username");
        assert_eq!(resolved.request.body_fields[0].value, "Ada Lovelace");
        assert_eq!(
            resolved.request.body_fields[1].value,
            "/tmp/private-token.txt"
        );
        assert_eq!(resolved.request.body_fields[2], template.body_fields[2]);
        assert_eq!(resolved.sensitive_values, vec!["private-token"]);
    }

    #[test]
    fn none_body_mode_ignores_stale_raw_and_structured_templates() {
        let mut request = RequestDraft::new("POST", "https://example.com/no-body");
        request.body_mode = BodyMode::None;
        request.body = "{{missing_raw}}".to_owned();
        request.body_fields = vec![BodyField::text(
            "{{missing_field_name}}",
            "{{missing_field_value}}",
        )];

        let resolved = resolve_request(&request, None).unwrap();

        assert_eq!(resolved.request.body, request.body);
        assert_eq!(resolved.request.body_fields, request.body_fields);
        assert!(resolved.used_variables.is_empty());
    }

    #[test]
    fn raw_body_mode_resolves_raw_and_ignores_stale_structured_templates() {
        let environment = environment(vec![variable("payload", "resolved", true, false)]);
        let mut request = RequestDraft::new("POST", "https://example.com/raw");
        request.body_mode = BodyMode::Raw;
        request.body = "{{payload}}".to_owned();
        request.body_fields = vec![BodyField::text(
            "{{missing_field_name}}",
            "{{missing_field_value}}",
        )];

        let resolved = resolve_request(&request, Some(&environment)).unwrap();

        assert_eq!(resolved.request.body, "resolved");
        assert_eq!(resolved.request.body_fields, request.body_fields);
        assert_eq!(resolved.used_variables, vec!["payload"]);
    }

    #[test]
    fn structured_body_modes_resolve_fields_and_ignore_stale_raw_template() {
        let environment = environment(vec![
            variable("field_name", "username", true, false),
            variable("field_value", "Ada", true, false),
        ]);

        for mode in [BodyMode::FormUrlEncoded, BodyMode::MultipartFormData] {
            let mut disabled =
                BodyField::text("{{missing_disabled_name}}", "{{missing_disabled_value}}");
            disabled.enabled = false;
            let mut request = RequestDraft::new("POST", "https://example.com/form");
            request.body_mode = mode;
            request.body = "{{missing_raw}}".to_owned();
            request.body_fields = vec![
                BodyField::text("{{field_name}}", "{{field_value}}"),
                disabled.clone(),
            ];

            let resolved = resolve_request(&request, Some(&environment)).unwrap();

            assert_eq!(resolved.request.body, request.body);
            assert_eq!(resolved.request.body_fields[0].name, "username");
            assert_eq!(resolved.request.body_fields[0].value, "Ada");
            assert_eq!(resolved.request.body_fields[1], disabled);
            assert_eq!(resolved.used_variables, vec!["field_name", "field_value"]);
        }
    }

    #[test]
    fn active_body_representation_still_reports_missing_variables() {
        let mut raw = RequestDraft::new("POST", "https://example.com/raw");
        raw.body_mode = BodyMode::Raw;
        raw.body = "{{missing_raw}}".to_owned();
        assert_eq!(
            resolve_request(&raw, None),
            Err(VariableResolutionError::UndefinedVariable {
                field: TemplateField::Body,
                name: "missing_raw".to_owned(),
            })
        );

        let mut form = RequestDraft::new("POST", "https://example.com/form");
        form.body_mode = BodyMode::FormUrlEncoded;
        form.body_fields = vec![BodyField::text("name", "{{missing_field}}")];
        assert_eq!(
            resolve_request(&form, None),
            Err(VariableResolutionError::UndefinedVariable {
                field: TemplateField::BodyFieldValue(0),
                name: "missing_field".to_owned(),
            })
        );
    }

    #[test]
    fn resolves_nested_variables_and_tracks_nested_secret() {
        let environment = environment(vec![
            variable("base", "https://{{host}}", true, false),
            variable("host", "{{subdomain}}.example.com", true, false),
            variable("subdomain", "private", true, true),
        ]);

        let resolved = resolve_request(
            &RequestDraft::new("GET", "{{base}}/health"),
            Some(&environment),
        )
        .unwrap();

        assert_eq!(resolved.request.url, "https://private.example.com/health");
        assert_eq!(resolved.sensitive_values, vec!["private"]);
        assert_eq!(resolved.used_variables, vec!["base", "host", "subdomain"]);
    }

    #[test]
    fn resolves_enabled_query_param_metadata_and_ignores_disabled_rows() {
        let environment = environment(vec![variable("account_id", "42", true, false)]);
        let mut request =
            RequestDraft::new("GET", "https://example.com/users?account={{account_id}}");
        let mut disabled = QueryParamEntry::new("debug", "{{missing}}");
        disabled.enabled = false;
        request.query_params.push(disabled);

        let resolved = resolve_request(&request, Some(&environment)).unwrap();
        assert_eq!(resolved.request.url, "https://example.com/users?account=42");
        assert_eq!(resolved.request.query_params[0].value, "42");
        assert_eq!(resolved.request.query_params[1].value, "{{missing}}");
        assert_eq!(resolved.used_variables, vec!["account_id"]);
    }

    #[test]
    fn redacts_raw_and_percent_encoded_secret_values() {
        let resolved = ResolvedRequest {
            request: RequestDraft::default(),
            used_variables: Vec::new(),
            sensitive_values: vec!["a b/c".to_owned()],
        };

        assert_eq!(
            resolved.redact_secrets("raw=a b/c form=a+b%2Fc url=a%20b/c path=a%20b%2Fc"),
            "raw=[REDACTED] form=[REDACTED] url=[REDACTED] path=[REDACTED]"
        );
    }

    #[test]
    fn ignores_placeholders_in_disabled_headers() {
        let mut disabled = HeaderEntry::new("Authorization", "{{missing}}");
        disabled.enabled = false;
        let request = RequestDraft {
            method: "GET".to_owned(),
            url: "https://example.com".to_owned(),
            headers: vec![disabled.clone()],
            body: String::new(),
            ..RequestDraft::default()
        };

        let resolved = resolve_request(&request, None).unwrap();
        assert_eq!(resolved.request.headers, vec![disabled]);
    }

    #[test]
    fn reports_missing_and_disabled_variables_separately() {
        let environment = environment(vec![variable("disabled", "value", false, false)]);

        assert_eq!(
            resolve_request(
                &RequestDraft::new("GET", "https://example.com/{{missing}}"),
                Some(&environment)
            ),
            Err(VariableResolutionError::UndefinedVariable {
                field: TemplateField::Url,
                name: "missing".to_owned(),
            })
        );
        assert_eq!(
            resolve_request(
                &RequestDraft::new("GET", "https://example.com/{{disabled}}"),
                Some(&environment)
            ),
            Err(VariableResolutionError::DisabledVariable {
                field: TemplateField::Url,
                name: "disabled".to_owned(),
            })
        );
    }

    #[test]
    fn detects_duplicate_enabled_variables() {
        let environment = environment(vec![
            variable("base", "one", true, false),
            EnvironmentVariable {
                id: "variable-base-two".to_owned(),
                ..variable("base", "two", true, false)
            },
        ]);

        assert_eq!(
            resolve_request(
                &RequestDraft::new("GET", "https://example.com"),
                Some(&environment)
            ),
            Err(VariableResolutionError::DuplicateVariable {
                name: "base".to_owned()
            })
        );
    }

    #[test]
    fn detects_variable_cycles() {
        let environment = environment(vec![
            variable("a", "{{b}}", true, false),
            variable("b", "{{c}}", true, false),
            variable("c", "{{a}}", true, false),
        ]);

        assert_eq!(
            resolve_request(
                &RequestDraft::new("GET", "https://example.com/{{a}}"),
                Some(&environment)
            ),
            Err(VariableResolutionError::VariableCycle {
                field: TemplateField::Url,
                chain: "a -> b -> c -> a".to_owned(),
            })
        );
    }

    #[test]
    fn reports_malformed_placeholders_with_field_and_offset() {
        let request = RequestDraft {
            method: "POST".to_owned(),
            url: "https://example.com".to_owned(),
            headers: Vec::new(),
            body: "before {{ name".to_owned(),
            ..RequestDraft::default()
        };
        assert_eq!(
            resolve_request(&request, None),
            Err(VariableResolutionError::UnclosedPlaceholder {
                field: TemplateField::Body,
                offset: 7,
            })
        );

        let request = RequestDraft {
            method: "POST".to_owned(),
            url: "https://example.com".to_owned(),
            headers: Vec::new(),
            body: "before {{   }} after".to_owned(),
            ..RequestDraft::default()
        };
        assert_eq!(
            resolve_request(&request, None),
            Err(VariableResolutionError::EmptyPlaceholder {
                field: TemplateField::Body,
                offset: 7,
            })
        );
    }

    #[test]
    fn treats_standalone_closing_braces_as_literal_text() {
        let request = RequestDraft {
            method: "POST".to_owned(),
            url: "https://example.com/}}".to_owned(),
            headers: vec![HeaderEntry::new("X-Literal", "prefix}}suffix")],
            body: r#"{"outer":{"inner":{"value":1}}}"#.to_owned(),
            ..RequestDraft::default()
        };

        let resolved = resolve_request(&request, None).unwrap();

        assert_eq!(resolved.request.url, request.url);
        assert_eq!(resolved.request.headers, request.headers);
        assert_eq!(resolved.request.body, request.body);
    }

    #[test]
    fn resolves_placeholders_inside_nested_json_with_adjacent_object_closers() {
        let environment = environment(vec![variable("item_id", "42", true, false)]);
        let request = RequestDraft {
            method: "POST".to_owned(),
            url: "https://example.com".to_owned(),
            headers: Vec::new(),
            body: r#"{"outer":{"inner":{"item":"{{item_id}}"}}}"#.to_owned(),
            ..RequestDraft::default()
        };

        let resolved = resolve_request(&request, Some(&environment)).unwrap();

        assert_eq!(
            resolved.request.body,
            r#"{"outer":{"inner":{"item":"42"}}}"#
        );
        assert_eq!(resolved.used_variables, vec!["item_id"]);
    }

    #[test]
    fn redact_secret_bytes_matches_legacy_windowed_scan() {
        // Literal copy of the pre-memmem `replace_bytes` (hand-rolled windowed
        // scan) so byte-for-byte equivalence of the optimized path is proven.
        fn legacy_replace_bytes(bytes: &[u8], needle: &[u8], replacement: &[u8]) -> Vec<u8> {
            if needle.is_empty() {
                return bytes.to_vec();
            }
            let mut output = Vec::with_capacity(bytes.len());
            let mut cursor = 0;
            while let Some(position) = bytes[cursor..]
                .windows(needle.len())
                .position(|candidate| candidate == needle)
            {
                let position = cursor + position;
                output.extend_from_slice(&bytes[cursor..position]);
                output.extend_from_slice(replacement);
                cursor = position + needle.len();
            }
            output.extend_from_slice(&bytes[cursor..]);
            output
        }

        let cases: &[(&[u8], &[&str])] = &[
            (b"", &["secret"]),
            (b"prefix secret suffix", &["secret"]),
            (b"secret at start", &["secret"]),
            (b"end secret", &["secret"]),
            (b"secret secret secret", &["secret"]),
            (b"adjacent secretssecret", &["secret"]),
            (b"no match here", &["secret"]),
            (b"key=abcd token=abc", &["abcd", "abc"]),
            (b"xabcd tail", &["abc", "abcd"]),
            (b"abczabcd", &["abcd", "abc"]),
            ("héllo wörld".as_bytes(), &["héllo"]),
            (b"", &[""]),
            (b"plain", &[""]),
            (b"", &[]),
        ];
        for (input, secret_values) in cases {
            let secret_values: Vec<String> = secret_values.iter().map(|s| s.to_string()).collect();
            let variants = secret_variants(&secret_values);
            let mut legacy = input.to_vec();
            let mut current = input.to_vec();
            for variant in &variants {
                legacy =
                    legacy_replace_bytes(&legacy, variant.as_bytes(), REDACTED_VALUE.as_bytes());
                current = replace_bytes(&current, variant.as_bytes(), REDACTED_VALUE.as_bytes());
            }
            assert_eq!(
                current, legacy,
                "byte output diverged for input {input:?} with secrets {secret_values:?}"
            );
        }
    }

    #[test]
    fn redact_secret_bytes_handles_overlapping_empty_and_utf8_borders() {
        // Overlapping secrets: longest-first replacement must not leave a
        // shorter secret spelling inside a replaced span.
        let out = redact_secret_bytes(
            b"key=abcd token=abc",
            &["abcd".to_owned(), "abc".to_owned()],
        );
        assert_eq!(out, b"key=[REDACTED] token=[REDACTED]");

        // Empty secrets list / empty needle are no-ops that copy the input.
        assert_eq!(redact_secret_bytes(b"unchanged", &[]), b"unchanged");
        assert_eq!(
            redact_secret_bytes(b"unchanged", &[String::new()]),
            b"unchanged"
        );

        // Multi-byte UTF-8 secrets scrub on byte boundaries in their raw,
        // form-encoded, and path-encoded spellings.
        let secret = "héllo wörld".to_owned();
        let haystack =
            format!("raw={secret} path=h%C3%A9llo%20w%C3%B6rld form=h%C3%A9llo+w%C3%B6rld");
        let out = redact_secret_bytes(haystack.as_bytes(), &[secret]);
        assert_eq!(out, b"raw=[REDACTED] path=[REDACTED] form=[REDACTED]");
        assert_ne!(out, haystack.as_bytes());
        let utf8 = String::from_utf8(out).unwrap();
        assert!(!utf8.contains("h%C3%A9llo"));

        // A secret that only starts or ends at a buffer border is still
        // scrubbed completely, with no truncated replacement.
        assert_eq!(
            redact_secret_bytes(b"secret-tail", &["secret".to_owned()]),
            b"[REDACTED]-tail"
        );
        assert_eq!(
            redact_secret_bytes(b"head-secret", &["secret".to_owned()]),
            b"head-[REDACTED]"
        );
    }

    #[test]
    fn secret_variants_are_sorted_longest_first_and_deduplicated() {
        let variants = secret_variants(&["abcd".to_owned(), "abc".to_owned()]);
        assert_eq!(variants, vec!["abcd".to_owned(), "abc".to_owned()]);

        // Callers feed `secret_variants` already-deduplicated values (the
        // redactor deduplicates on insert); for one such value every distinct
        // spelling is present, longest-first: fully path-encoded segment
        // spelling, form-encoded, path-encoded (space only), raw. The
        // query-encoded spelling (`a%20b/c`) collides with the path spelling
        // and is collapsed by the adjacent `dedup()`.
        let variants = secret_variants(&["a b/c".to_owned()]);
        assert_eq!(variants, vec!["a%20b%2Fc", "a+b%2Fc", "a%20b/c", "a b/c"]);
        assert_eq!(variants.iter().map(String::len).max(), Some(9));

        assert!(secret_variants(&[]).is_empty());
        assert!(secret_variants(&[String::new()]).is_empty());
    }

    #[test]
    fn request_template_deserializes_without_scripts() {
        let json = r#"{
            "request": {
                "method": "GET",
                "url": "https://example.com",
                "headers": [],
                "body": ""
            }
        }"#;
        let template: RequestTemplate = serde_json::from_str(json).unwrap();
        assert_eq!(template.scripts, RequestScripts::default());
    }
}

#[cfg(test)]
mod documentation_tests {
    use super::*;

    #[test]
    fn documentation_is_optional_and_round_trips_for_both_protocols() {
        let legacy: RequestTemplate = serde_json::from_str("{}").unwrap();
        assert!(legacy.documentation.is_empty());
        for mut template in [legacy, RequestTemplate::websocket(Default::default())] {
            template.documentation = "# Notes\nUnicode: café".to_owned();
            let json = serde_json::to_string(&template).unwrap();
            assert_eq!(
                serde_json::from_str::<RequestTemplate>(&json).unwrap(),
                template
            );
        }
    }
}

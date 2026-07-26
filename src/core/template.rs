use std::collections::{HashMap, HashSet};
use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::request::RequestDraft;
use super::workspace::{Environment, RequestScripts};

const MAX_VARIABLE_DEPTH: usize = 32;
const REDACTED_VALUE: &str = "[REDACTED]";

/// A persistable request definition.
///
/// The request remains an unexpanded template. Variable expansion produces a
/// separate [`ResolvedRequest`] immediately before the network request starts.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RequestTemplate {
    #[serde(default)]
    pub request: RequestDraft,
    #[serde(default)]
    pub scripts: RequestScripts,
}

impl RequestTemplate {
    pub fn new(request: RequestDraft) -> Self {
        Self {
            request,
            scripts: RequestScripts::default(),
        }
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

    variants.into_iter().fold(text.to_owned(), |text, value| {
        text.replace(&value, REDACTED_VALUE)
    })
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
/// Disabled header rows are deliberately left untouched because the request
/// layer does not send them. This lets users keep incomplete templated headers
/// disabled without blocking an otherwise valid request.
pub fn resolve_request(
    template: &RequestDraft,
    environment: Option<&Environment>,
) -> Result<ResolvedRequest, VariableResolutionError> {
    let mut resolver = Resolver::new(environment)?;
    let mut request = template.clone();

    request.url = resolver.resolve_text(&request.url, TemplateField::Url)?;
    for (index, header) in request.headers.iter_mut().enumerate() {
        if !header.enabled {
            continue;
        }
        header.name = resolver.resolve_text(&header.name, TemplateField::HeaderName(index))?;
        header.value = resolver.resolve_text(&header.value, TemplateField::HeaderValue(index))?;
    }
    request.body = resolver.resolve_text(&request.body, TemplateField::Body)?;
    for (index, field) in request.body_fields.iter_mut().enumerate() {
        if !field.enabled {
            continue;
        }
        field.name = resolver.resolve_text(&field.name, TemplateField::BodyFieldName(index))?;
        field.value = resolver.resolve_text(&field.value, TemplateField::BodyFieldValue(index))?;
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
    use crate::core::request::{BodyField, BodyFieldKind, BodyMode, HeaderEntry};
    use crate::core::workspace::EnvironmentVariable;

    fn environment(variables: Vec<EnvironmentVariable>) -> Environment {
        Environment {
            id: "environment-test".to_owned(),
            name: "Test".to_owned(),
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

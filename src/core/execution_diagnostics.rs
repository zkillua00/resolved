//! Shared diagnostics for the HTTP and WebSocket proxy protocols.
//!
//! Describe malformed payloads by shape, not by dumping their values: errors
//! reach history, scripts and logs, where response bodies and credentials do
//! not belong. Structured server errors are already part of the public API.

use std::{collections::BTreeMap, error::Error as _};

use reqwest::{StatusCode, header::HeaderMap};
use serde::Deserialize;
use serde_json::Value;

#[derive(Default, Deserialize)]
#[serde(default)]
pub(super) struct ServerError {
    pub code: String,
    pub message: String,
    pub fields: BTreeMap<String, String>,
    pub phase: String,
    pub reason: String,
}

impl ServerError {
    pub fn summary(&self) -> String {
        let mut message = if self.message.trim().is_empty() {
            "the server rejected the request".to_owned()
        } else {
            bounded_text(&self.message)
        };
        let fields = self
            .fields
            .iter()
            .filter(|(_, reason)| !reason.trim().is_empty())
            .take(16)
            .map(|(field, reason)| {
                // `request` is exact action state for the allowlist, not display
                // text. Never strip it on the wire or matching semantics change.
                let detail = if self.code == "proxy_destination_blocked" && field == "request" {
                    diagnostic_request_url(reason)
                } else {
                    bounded_text(reason)
                };
                format!("{}: {detail}", bounded_text(field))
            })
            .collect::<Vec<_>>();
        if !fields.is_empty() {
            message.push_str(&format!(" ({})", fields.join(", ")));
        }
        if self.fields.len() > 16 {
            message.push_str(" (additional validation fields omitted)");
        }
        let details = [
            ("code", &self.code),
            ("phase", &self.phase),
            ("reason", &self.reason),
        ]
        .into_iter()
        .filter(|(_, value)| !value.trim().is_empty())
        .map(|(name, value)| format!("{name}: {}", bounded_text(value)))
        .collect::<Vec<_>>();
        if !details.is_empty() {
            message.push_str(&format!(" [{}]", details.join("; ")));
        }
        message
    }
}

fn diagnostic_request_url(value: &str) -> String {
    let Ok(mut url) = url::Url::parse(value) else {
        return "<invalid URL; contents omitted>".into();
    };
    let omitted = !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some();
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_query(None);
    url.set_fragment(None);
    let label = bounded_text(url.as_str());
    if omitted {
        format!("{label} (credentials/query/fragment omitted)")
    } else {
        label
    }
}

#[derive(Deserialize)]
pub(super) struct ErrorEnvelope {
    #[serde(default)]
    pub request_id: String,
    pub error: Option<ServerError>,
}

pub(super) struct ResponseContext {
    pub status: StatusCode,
    content_type: String,
    request_id: String,
}

impl ResponseContext {
    pub fn from_http(status: StatusCode, headers: &HeaderMap) -> Self {
        Self {
            status,
            // MIME parameters can contain arbitrary values; only the media type
            // is needed to distinguish a JSON envelope from a login/HTML page.
            content_type: headers
                .get("content-type")
                .map(|value| {
                    value
                        .to_str()
                        .map(|value| bounded_text(value.split(';').next().unwrap_or("").trim()))
                        .unwrap_or_else(|_| "<invalid header encoding>".into())
                })
                .unwrap_or_else(|| "<missing>".into()),
            request_id: headers
                .get("x-request-id")
                .and_then(|value| value.to_str().ok())
                .map(bounded_text)
                .unwrap_or_default(),
        }
    }

    pub fn describe(&self) -> String {
        self.describe_with_request_id("")
    }

    fn describe_with_request_id(&self, request_id: &str) -> String {
        let mut description = format!("HTTP {}; Content-Type: {}", self.status, self.content_type);
        let request_id = if request_id.trim().is_empty() {
            &self.request_id
        } else {
            request_id
        };
        if !request_id.is_empty() {
            description.push_str(&format!("; request ID: {}", bounded_text(request_id)));
        }
        description
    }

    pub fn failure(&self, operation: &str, error: &ServerError, request_id: &str) -> String {
        format!(
            "{operation} failed: {} ({})",
            error.summary(),
            self.describe_with_request_id(request_id)
        )
    }

    pub fn invalid(
        &self,
        operation: &str,
        expected: &str,
        body: &[u8],
        reason: Option<&str>,
    ) -> String {
        invalid_response(
            operation,
            expected,
            &format!("{}; {}", self.describe(), describe_body(body)),
            reason,
        )
    }
}

pub(super) fn invalid_response(
    operation: &str,
    expected: &str,
    received: &str,
    reason: Option<&str>,
) -> String {
    let mut message = format!(
        "{operation} returned an invalid response; expected {expected}; received {received}"
    );
    if let Some(reason) = reason.filter(|reason| !reason.is_empty()) {
        message.push_str(&format!("; {reason}"));
    }
    message
}

/// Limit server-supplied diagnostic text, preserving UTF-8 and escaping controls.
/// This is not a secret redactor; callers must not pass raw payload values.
pub(super) fn bounded_text(text: &str) -> String {
    let mut escaped = text.chars().flat_map(|character| {
        if character.is_control() {
            character.escape_default().collect::<Vec<_>>()
        } else {
            vec![character]
        }
    });
    let mut result: String = escaped.by_ref().take(512).collect();
    if escaped.next().is_some() {
        result.push_str("… [truncated]");
    }
    result
}

pub(super) fn describe_body(body: &[u8]) -> String {
    if body.is_empty() {
        return "empty body (0 bytes)".into();
    }
    // Don't allocate a second, potentially huge response just for a diagnostic.
    if body.len() <= 64 * 1024
        && let Ok(value) = serde_json::from_slice::<Value>(body)
    {
        return format!(
            "JSON {} ({} bytes; values omitted)",
            bounded_text(&json_shape(&value, 0)),
            body.len()
        );
    }
    let kind = match std::str::from_utf8(body) {
        Ok(text) if text.trim_start().starts_with('<') => "HTML/XML",
        Ok(text) if matches!(text.trim_start().chars().next(), Some('{' | '[')) => {
            if body.len() > 64 * 1024 {
                "JSON-like body (too large for a diagnostic shape)"
            } else {
                "malformed JSON"
            }
        }
        Ok(_) => "non-JSON text",
        Err(_) => "non-UTF-8 body",
    };
    format!("{kind} ({} bytes; contents omitted)", body.len())
}

fn json_shape(value: &Value, depth: usize) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(value) => value.to_string(),
        Value::Number(_) => "number".into(),
        Value::String(value) => format!("string ({} bytes)", value.len()),
        Value::Array(values) if depth < 3 => match values.first() {
            Some(first) => format!("[{}; {} items]", json_shape(first, depth + 1), values.len()),
            None => "[]".into(),
        },
        Value::Object(fields) if depth < 3 => {
            let mut entries = fields
                .iter()
                .take(12)
                .map(|(key, value)| {
                    // Arbitrary response keys can themselves contain credentials.
                    let name = if is_protocol_field(key) {
                        key.as_str()
                    } else {
                        "<unknown field>"
                    };
                    format!("{name:?}: {}", json_shape(value, depth + 1))
                })
                .collect::<Vec<_>>();
            if fields.len() > 12 {
                entries.push(format!("… {} more fields", fields.len() - 12));
            }
            format!("{{{}}}", entries.join(", "))
        }
        Value::Array(values) => format!("array ({} items)", values.len()),
        Value::Object(fields) => format!("object ({} fields)", fields.len()),
    }
}

fn is_protocol_field(key: &str) -> bool {
    matches!(
        key,
        "success"
            | "data"
            | "error"
            | "request_id"
            | "type"
            | "message"
            | "code"
            | "fields"
            | "phase"
            | "reason"
            | "subprotocol"
            | "status"
            | "status_text"
            | "http_version"
            | "final_url"
            | "headers"
            | "name"
            | "value"
            | "body_base64"
            | "content_type"
            | "duration_micros"
            | "mode"
            | "cookie_jar"
            | "limits"
            | "unlimited"
    ) || super::execution_limits::DEFAULT_LIMITS
        .iter()
        .any(|(name, _)| *name == key)
}

/// serde's data errors can quote an entire string (including a token). Keep
/// schema expectations and locations without echoing the offending value.
pub(super) fn json_error_reason(error: &serde_json::Error) -> String {
    let message = error.to_string();
    if !error.is_data() || message.starts_with("missing field `") {
        return bounded_text(&message);
    }
    if let Some((received, expected)) = message.rsplit_once(", expected ") {
        let kind = if received.starts_with("invalid type: string") {
            "string"
        } else if received.starts_with("invalid type: integer") {
            "integer"
        } else if received.starts_with("invalid type: floating point") {
            "floating point number"
        } else if received.starts_with("invalid type: boolean") {
            "boolean"
        } else if received.starts_with("invalid type: null") {
            "null"
        } else if received.starts_with("invalid type: sequence") {
            "array"
        } else if received.starts_with("invalid type: map") {
            "object"
        } else {
            "value of the wrong type or outside the allowed range"
        };
        return format!("received {kind}, expected {}", bounded_text(expected));
    }
    format!(
        "JSON schema mismatch at line {} column {}",
        error.line(),
        error.column()
    )
}

pub(super) fn transport_failure(operation: &str, error: reqwest::Error) -> String {
    // reqwest's Display otherwise includes the entire URL. Its underlying
    // sources carry the actual DNS/connect/TLS/body-read cause.
    let error = error.without_url();
    let mut reasons = vec![bounded_text(&error.to_string())];
    let mut source = error.source();
    for _ in 0..8 {
        let Some(cause) = source else { break };
        let reason = bounded_text(&cause.to_string());
        if !reasons.contains(&reason) {
            reasons.push(reason);
        }
        source = cause.source();
    }
    format!("{operation} failed: {}", reasons.join(": "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_errors_keep_fields_codes_and_phases_even_without_a_message() {
        let error: ServerError = serde_json::from_str(
            r#"{"code":"validation_failed","fields":{"url":"must use ws or wss","headers":"invalid name"},"phase":"validation","reason":"invalid_field"}"#,
        )
        .unwrap();
        assert_eq!(
            error.summary(),
            "the server rejected the request (headers: invalid name, url: must use ws or wss) [code: validation_failed; phase: validation; reason: invalid_field]"
        );
    }

    #[test]
    fn malformed_response_summaries_show_shape_not_payload_values() {
        let summary = describe_body(
            br#"{"success":true,"data":{"body_base64":"private-payload","headers":[{"name":"Authorization","value":"Bearer secret"}],"status":"not-a-number"}}"#,
        );
        assert!(summary.contains("\"status\": string"), "{summary}");
        assert!(summary.contains("\"success\": true"), "{summary}");
        for secret in ["private-payload", "Bearer secret", "not-a-number"] {
            assert!(!summary.contains(secret), "{summary}");
        }
        let summary = describe_body(br#"{"private-token":{"data":{"private-password":true}}}"#);
        assert!(!summary.contains("private-token"), "{summary}");
        assert!(!summary.contains("private-password"), "{summary}");
        assert!(summary.contains("<unknown field>"), "{summary}");
        let summary = describe_body(b"<html>private-session-token</html>");
        assert!(summary.contains("HTML/XML"));
        assert!(!summary.contains("private-session-token"));
        assert_eq!(describe_body(b""), "empty body (0 bytes)");
    }

    #[test]
    fn json_type_errors_do_not_echo_secret_strings() {
        let error = serde_json::from_str::<u16>(r#""private-secret""#).unwrap_err();
        let reason = json_error_reason(&error);
        assert!(reason.contains("received string, expected u16"), "{reason}");
        assert!(!reason.contains("private-secret"));
    }

    #[test]
    fn diagnostic_bounds_preserve_unicode_and_escape_controls() {
        let text = bounded_text(&format!("hello\n{}", "🦀".repeat(1000)));
        assert!(text.starts_with("hello\\n"));
        assert!(text.ends_with("… [truncated]"));
        assert!(!text.contains('\n'));
        assert!(text.chars().count() < 550);
    }

    #[test]
    fn response_context_uses_envelope_request_id_then_header_fallback() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "content-type",
            "application/json; private=secret".parse().unwrap(),
        );
        headers.insert("x-request-id", "header-id".parse().unwrap());
        let context = ResponseContext::from_http(StatusCode::UNPROCESSABLE_ENTITY, &headers);
        assert!(context.describe().contains("HTTP 422"));
        assert!(context.describe().contains("request ID: header-id"));
        assert!(!context.describe().contains("secret"));
        let message = context.failure("proxy execution", &ServerError::default(), "body-id");
        assert!(message.contains("request ID: body-id"));
        assert!(!message.contains("header-id"));
    }
}

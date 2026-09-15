use super::*;

fn exchange(body: &[u8]) -> McpHttpExchangeSnapshot {
    McpHttpExchangeSnapshot {
        operation_id: 42,
        history_entry_ids: vec![],
        sensitive_values: vec!["private token".into(), "quoted\"secret".into(), "98765".into()],
        state: "completed",
        stage: None,
        request: Some(RequestDraft {
            url: "https://example.com/private%20token".into(),
            headers: vec![HeaderEntry::new("X-Custom", "Bearer private token")],
            body: r#"{"value":"quoted\"secret","public":"visible"}"#.into(),
            ..RequestDraft::default()
        }),
        response: Some(ResponseData {
            status: 200,
            status_text: "OK".into(),
            http_version: "HTTP/1.1".into(),
            final_url: "https://example.com/?value=private+token".into(),
            headers: vec![],
            content_type: Some("application/json".into()),
            body: body.to_vec().into(),
            duration: Duration::from_millis(1),
        }),
        error: Some("failed: private token".into()),
        diagnostic: None,
        pre_request_report: Some(ScriptReport {
            phase: ScriptPhase::PreRequest,
            duration: Duration::ZERO,
            logs: vec![ScriptLog {
                level: ScriptLogLevel::Log,
                message: "private token".into(),
                values: vec![],
            }],
            tests: vec![],
            response_body_truncated: false,
        }),
        post_response_report: None,
    }
}

#[test]
fn mcp_redaction_covers_exchange_without_changing_wire_snapshot() {
    let body = br#"{"private token":"quoted\"secret","number":98765,"public":"visible"}"#;
    let snapshot = exchange(body);
    let result = http_exchange_value(&snapshot, usize::MAX);
    let serialized = result.to_string();
    for secret in ["private token", "private%20token", "private+token", "quoted", "98765"] {
        assert!(!serialized.contains(secret), "leaked {secret}: {serialized}");
    }
    assert_eq!(result["request"]["headers"][0]["value"], "Bearer [REDACTED]");
    let response: Value = serde_json::from_str(result["response"]["body"].as_str().unwrap()).unwrap();
    assert_eq!(response["public"], "visible");
    assert_eq!(response["[REDACTED]"], "[REDACTED]");
    assert_eq!(response["number"], "[REDACTED]");
    assert_eq!(snapshot.response.as_ref().unwrap().body.as_ref(), body);
    assert_eq!(snapshot.request.as_ref().unwrap().headers[0].value, "Bearer private token");
}

#[test]
fn mcp_redaction_precedes_truncation_and_binary_encoding() {
    let snapshot = exchange(b"private token suffix");
    let result = http_exchange_value(&snapshot, 7);
    assert_eq!(result["response"]["body"], "[REDACT");
    assert_eq!(result["response"]["body_truncated"], true);
    assert_eq!(result["response"]["body_included_bytes"], 7);

    let snapshot = exchange(b"\xffprivate token suffix");
    let result = http_exchange_value(&snapshot, usize::MAX);
    assert_eq!(result["response"]["body_encoding"], "base64");
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(result["response"]["body"].as_str().unwrap())
        .unwrap();
    assert_eq!(decoded, b"\xff[REDACTED] suffix");
}

#[test]
fn mcp_redaction_queries_decoded_values_and_errors() {
    let snapshot = exchange(br#"{"items":[{"value":"quoted\"secret","number":98765,"public":true}]}"#);
    let query = |params| ApiTester::query_http_response_value(
        params, snapshot.operation_id, snapshot.response.as_ref(), &snapshot.sensitive_values,
    );
    let result = query(json!({
        "json_pointer": "/items",
        "projection": {"value": "/value", "number": "/number", "public": "/public"}
    })).unwrap();
    assert_eq!(result["value"], json!([{
        "value": "[REDACTED]", "number": "[REDACTED]", "public": true
    }]));
    assert_eq!(result["output_bytes"], serde_json::to_vec(&result["value"]).unwrap().len());
    let error = query(json!({"json_pointer": "/private token"})).unwrap_err();
    assert!(!error.contains("private token"));
}

#[test]
fn mcp_redaction_keeps_public_response_bytes_unchanged() {
    let body = b"{ \"public\": \"visible\", \"count\": 123 }\n";
    assert_eq!(redact_mcp_body(body, &["".into(), "not present".into()]), body);
}

#[test]
fn mcp_redaction_covers_json_literal_secrets_and_websocket_escapes() {
    let secrets = vec!["true".into(), "false".into(), "null".into()];
    let mut value = json!({"string": "true", "boolean": true, "false": false, "null": null});
    redact_mcp_value(&mut value, &secrets);
    assert_eq!(value["boolean"], "[REDACTED]");
    assert_eq!(value["string"], "[REDACTED]");
    assert!(!value.to_string().contains("true"));

    let event = ControlWebSocketEvent {
        id: 1,
        at: Utc::now(),
        direction: "received",
        kind: "text",
        payload: Some(r#"{"value":"\u0070rivate token"}"#.into()),
        binary: None,
    };
    let value = control_websocket_event_value(&event, usize::MAX, &["private token".into()]);
    assert_eq!(value["payload"], r#"{"value":"[REDACTED]"}"#);
}

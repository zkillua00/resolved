use super::script_console::*;
use super::*;
use crate::core::{ScriptLog, ScriptTestResult};

#[test]
fn raw_json_formatter_pretty_prints_without_changing_values() {
    let formatted = format_raw_body_source(
        RawBodyLanguage::Json,
        r#"{"nested":{"ok":true},"items":[1,2]}"#,
        &Default::default(),
    )
    .unwrap();
    assert_eq!(
        formatted,
        "{\n  \"nested\": {\n    \"ok\": true\n  },\n  \"items\": [\n    1,\n    2\n  ]\n}"
    );
}

#[test]
fn raw_formatter_does_not_guess_for_unsupported_languages() {
    let source = "name: value";
    let error =
        format_raw_body_source(RawBodyLanguage::Yaml, source, &Default::default()).unwrap_err();
    assert!(error.contains("not available for YAML"));
    assert_eq!(source, "name: value");
}

#[test]
fn script_console_builds_level_and_test_rows_with_stable_copy_text() {
    let report = ScriptReport {
        phase: ScriptPhase::PreRequest,
        duration: Duration::from_micros(420),
        logs: vec![
            ScriptLog {
                level: ScriptLogLevel::Log,
                message: "plain".to_owned(),
            },
            ScriptLog {
                level: ScriptLogLevel::Info,
                message: "bilgi 🧪".to_owned(),
            },
            ScriptLog {
                level: ScriptLogLevel::Warn,
                message: "careful".to_owned(),
            },
            ScriptLog {
                level: ScriptLogLevel::Error,
                message: "boom".to_owned(),
            },
            ScriptLog {
                level: ScriptLogLevel::Debug,
                message: "details".to_owned(),
            },
        ],
        tests: vec![
            ScriptTestResult {
                name: "created".to_owned(),
                passed: true,
                message: None,
            },
            ScriptTestResult {
                name: "has token".to_owned(),
                passed: false,
                message: Some("expected value\nreceived none".to_owned()),
            },
        ],
        response_body_truncated: true,
    };

    let model = script_console_model(Some(&report), None, None, None);
    assert_eq!(
        model.sections[0]
            .rows
            .iter()
            .map(|row| row.label.as_str())
            .collect::<Vec<_>>(),
        [
            "LOG", "INFO", "WARN", "ERROR", "DEBUG", "PASS", "FAIL", "NOTICE"
        ]
    );
    assert_eq!(model.sections[0].rows[1].copy_value, "bilgi 🧪");
    assert_eq!(
        model.sections[0].rows[6].copy_value,
        "[FAIL] has token\nexpected value\nreceived none"
    );
    assert_eq!(
        model.copy_all_text(),
        concat!(
            "Pre-request · 0.42 ms\n",
            "[LOG] plain\n",
            "[INFO] bilgi 🧪\n",
            "[WARN] careful\n",
            "[ERROR] boom\n",
            "[DEBUG] details\n",
            "[PASS] created\n",
            "[FAIL] has token\n",
            "    expected value\n",
            "    received none\n",
            "[NOTICE] Response body was truncated for the script runtime."
        )
    );
}

#[test]
fn script_console_keeps_diagnostics_copyable_without_an_http_response() {
    let report = ScriptReport {
        phase: ScriptPhase::PreRequest,
        duration: Duration::from_millis(3),
        logs: vec![ScriptLog {
            level: ScriptLogLevel::Info,
            message: "before failure".to_owned(),
        }],
        tests: Vec::new(),
        response_body_truncated: false,
    };
    let diagnostic = ScriptDiagnostic {
        phase: ScriptPhase::PreRequest,
        kind: ScriptErrorKind::Runtime,
        filename: "pre-request.js",
        message: "patladı".to_owned(),
        stack: Some("at pre-request.js:4\nat <eval>".to_owned()),
    };

    let model = script_console_model(
        Some(&report),
        None,
        Some(&diagnostic),
        Some("duplicate fallback error"),
    );
    assert_eq!(model.sections.len(), 1);
    assert_eq!(model.sections[0].rows.len(), 2);
    assert_eq!(
        model.sections[0].rows[1].copy_value,
        "[RUNTIME] pre-request.js: patladı\nat pre-request.js:4\nat <eval>"
    );
    let copied = model.copy_all_text();
    assert!(copied.contains("[INFO] before failure"));
    assert!(copied.contains("[RUNTIME] pre-request.js: patladı"));
    assert!(copied.contains("    at pre-request.js:4"));
    assert!(!copied.contains("duplicate fallback error"));
}

#[test]
fn script_console_includes_network_failure_after_pre_script_output() {
    let report = ScriptReport {
        phase: ScriptPhase::PreRequest,
        duration: Duration::from_millis(1),
        logs: Vec::new(),
        tests: Vec::new(),
        response_body_truncated: false,
    };

    let model = script_console_model(Some(&report), None, None, Some("connection refused"));
    assert_eq!(model.sections.len(), 2);
    assert_eq!(model.sections[0].rows[0].label, "EMPTY");
    assert_eq!(model.sections[1].title, "Request");
    assert_eq!(model.sections[1].rows[0].copy_value, "connection refused");
    assert!(model.copy_all_text().contains("[ERROR] connection refused"));
}

#[test]
fn script_console_keeps_pre_request_before_post_response() {
    let pre = ScriptReport {
        phase: ScriptPhase::PreRequest,
        duration: Duration::from_millis(1),
        logs: vec![ScriptLog {
            level: ScriptLogLevel::Log,
            message: "pre".to_owned(),
        }],
        tests: Vec::new(),
        response_body_truncated: false,
    };
    let post = ScriptReport {
        phase: ScriptPhase::PostResponse,
        duration: Duration::from_millis(2),
        logs: vec![ScriptLog {
            level: ScriptLogLevel::Info,
            message: "post".to_owned(),
        }],
        tests: Vec::new(),
        response_body_truncated: false,
    };

    let model = script_console_model(Some(&pre), Some(&post), None, None);
    assert_eq!(
        model
            .sections
            .iter()
            .map(|section| section.title.as_str())
            .collect::<Vec<_>>(),
        ["Pre-request", "Post-response"]
    );
    assert_eq!(
        model.copy_all_text(),
        "Pre-request · 1.00 ms\n[LOG] pre\n\n\
             Post-response · 2.00 ms\n[INFO] post"
    );
}

#[test]
fn empty_script_console_has_a_copyable_empty_state() {
    let model = script_console_model(None, None, None, None);
    assert_eq!(model.row_count(), 0);
    assert_eq!(model.copy_all_text(), "No script has run yet.");
}

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

#[test]
fn snippet_list_rows_filter_case_insensitively_and_order_by_category() {
    use super::snippets::{SnippetListRow, filter_snippet_list_rows};
    use crate::core::{SnippetCategory, SnippetKind};

    fn row(id: &str, name: &str, description: &str, category: SnippetCategory) -> SnippetListRow {
        SnippetListRow {
            id: id.to_owned(),
            name: name.to_owned(),
            description: description.to_owned(),
            category,
            kind: SnippetKind::Plain,
            name_lower: name.to_lowercase(),
            description_lower: description.to_lowercase(),
            category_label_lower: category.label().to_lowercase(),
        }
    }

    let rows = vec![
        row(
            "post-zed",
            "Zed fetch",
            "Posts a JSON body",
            SnippetCategory::PostResponse,
        ),
        row(
            "pre-zeal",
            "zeal",
            "Attach auth header",
            SnippetCategory::PreRequest,
        ),
        row(
            "pre-retry",
            "Retry",
            "zealous retry on 429",
            SnippetCategory::PreRequest,
        ),
        row(
            "post-header",
            "Header",
            "ADDS ZEALOUS HEADERS",
            SnippetCategory::PostResponse,
        ),
    ];

    fn ids(filtered: &[SnippetListRow]) -> Vec<&str> {
        filtered.iter().map(|row| row.id.as_str()).collect()
    }

    // Empty query keeps every snippet, ordered by category then name.
    assert_eq!(
        ids(&filter_snippet_list_rows("", rows.clone())),
        ["pre-retry", "pre-zeal", "post-header", "post-zed"]
    );

    // Case-insensitive match against the name only (mixed-case display text).
    assert_eq!(
        ids(&filter_snippet_list_rows("zed", rows.clone())),
        ["post-zed"]
    );

    // Case-insensitive match against descriptions, all-caps display text.
    assert_eq!(
        ids(&filter_snippet_list_rows("zealous", rows.clone())),
        ["pre-retry", "post-header"]
    );

    // Match against the category label, returning only that category.
    assert_eq!(
        ids(&filter_snippet_list_rows("response", rows.clone())),
        ["post-header", "post-zed"]
    );
}

#[gpui::test]
fn body_editor_folds_nested_json_via_keyboard_chords(cx: &mut gpui::TestAppContext) {
    use gpui::{px, size};

    let directory = tempfile::tempdir().expect("create temporary settings directory");
    let store = DatabaseStore::new(directory.path().join("api-tester.sqlite3"));
    store.initialize().expect("initialize test database");
    store
        .save_app_settings(&AppSettings::default())
        .expect("seed app settings");

    let mut app = None;
    let store_for_app = store.clone();
    let (_, cx) = cx.add_window_view(|window, cx| {
        gpui_component::init(cx);
        let base_key_bindings = shortcuts::capture_base_key_bindings(cx);
        crate::theme::configure(cx);
        let view = cx.new(|cx| {
            ApiTester::new_with_database_store(base_key_bindings, store_for_app, window, cx)
        });
        crate::register_app_action_handlers(&view, cx);
        app = Some(view.clone());
        gpui_component::Root::new(view, window, cx)
    });
    let app = app.expect("capture app entity");
    cx.update(|window, _| window.activate_window());
    cx.simulate_resize(size(px(1_200.), px(800.)));

    // Open a request, switch to the JSON body editor, and seed a nested doc.
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.workspace_tabs.activate_request();
            app.request_pane = RequestPane::Body;
            let json = "{\n  \"user\": {\n    \"name\": \"ada\"\n  }\n}";
            app.body.update(cx, |editor, cx| {
                editor.set_language(crate::code_editor::CodeLanguage::Json, cx);
                editor.set_value(json, window, cx);
            });
            app.body.read(cx).focus_handle(cx).focus(window);
            cx.notify();
        });
    });
    cx.run_until_parked();

    // After rendering, the syntax tree yields the nested regions, unfolded.
    let regions = cx.update(|_, cx| app.read(cx).body.read(cx).fold_regions(cx));
    assert_eq!(regions, vec![(0, 4, false), (1, 3, false)]);

    // Collapse everything fold-all (secondary-K secondary-0).
    cx.simulate_keystrokes("secondary-k secondary-0");
    cx.run_until_parked();
    assert!(
        cx.update(|_, cx| app.read(cx).body.read(cx).fold_regions(cx))
            .iter()
            .all(|(_, _, folded)| *folded),
        "⌘K ⌘0 collapses every region"
    );

    // Expand everything unfold-all (secondary-K secondary-J).
    cx.simulate_keystrokes("secondary-k secondary-j");
    cx.run_until_parked();
    assert!(
        cx.update(|_, cx| app.read(cx).body.read(cx).fold_regions(cx))
            .iter()
            .all(|(_, _, folded)| !*folded),
        "⌘K ⌘J expands every region"
    );
}

#[gpui::test]
fn params_editor_stays_in_sync_with_the_request_url(cx: &mut gpui::TestAppContext) {
    use gpui::{px, size};

    let directory = tempfile::tempdir().expect("create temporary settings directory");
    let store = DatabaseStore::new(directory.path().join("api-tester.sqlite3"));
    store.initialize().expect("initialize test database");

    let mut app = None;
    let store_for_app = store.clone();
    let (_, cx) = cx.add_window_view(|window, cx| {
        gpui_component::init(cx);
        let base_key_bindings = shortcuts::capture_base_key_bindings(cx);
        crate::theme::configure(cx);
        let view = cx.new(|cx| {
            ApiTester::new_with_database_store(base_key_bindings, store_for_app, window, cx)
        });
        app = Some(view.clone());
        gpui_component::Root::new(view, window, cx)
    });
    let app = app.expect("capture app entity");
    cx.update(|window, _| window.activate_window());
    cx.simulate_resize(size(px(1_200.), px(800.)));

    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.url.update(cx, |input, cx| {
                input.set_value(
                    "https://example.test/search?q=hello+world&page=2#results",
                    window,
                    cx,
                );
            });
        });
    });
    cx.run_until_parked();

    let (page_id, page_value, page_description) = cx.update(|_, cx| {
        let app = app.read(cx);
        assert_eq!(app.request_pane, RequestPane::Params);
        assert_eq!(app.request_query_param_count(cx), 2);
        assert_eq!(app.query_params[0].key.read(cx).value().as_ref(), "q");
        assert_eq!(
            app.query_params[0].value.read(cx).value().as_ref(),
            "hello world"
        );
        (
            app.query_params[1].id,
            app.query_params[1].value.clone(),
            app.query_params[1].description.clone(),
        )
    });

    cx.update(|window, cx| {
        page_value.update(cx, |input, cx| input.set_value("3", window, cx));
        page_description.update(cx, |input, cx| {
            input.set_value("Pagination cursor", window, cx)
        });
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert_eq!(
            app.read(cx).url.read(cx).value().as_ref(),
            "https://example.test/search?q=hello+world&page=3#results"
        );
        assert_eq!(
            app.read(cx).draft(cx).query_params[1].description,
            "Pagination cursor"
        );
    });

    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.toggle_query_param_row(page_id, false, window, cx);
        });
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        let app = app.read(cx);
        assert_eq!(
            app.url.read(cx).value().as_ref(),
            "https://example.test/search?q=hello+world#results"
        );
        assert!(!app.draft(cx).query_params[1].enabled);
        assert_eq!(
            app.draft(cx).query_params[1].description,
            "Pagination cursor"
        );
    });
}

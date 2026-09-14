use super::*;

#[gpui::test]
fn mcp_http_runs_overlap_and_keep_scoped_results(cx: &mut gpui::TestAppContext) {
    use std::io::{Read as _, Write as _};
    use std::net::TcpListener;
    use std::sync::{Arc, Barrier, mpsc};
    use std::time::{Duration, Instant};

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let arrived = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let barrier = Arc::new(Barrier::new(2));
    let (release_slow, slow_wait) = mpsc::channel();
    let slow_wait = Arc::new(std::sync::Mutex::new(slow_wait));
    let server_arrived = arrived.clone();
    let server = std::thread::spawn(move || {
        let mut workers = Vec::new();
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            let barrier = barrier.clone();
            let arrived = server_arrived.clone();
            let slow_wait = slow_wait.clone();
            workers.push(std::thread::spawn(move || {
                stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
                let mut request = [0; 4096];
                let len = stream.read(&mut request).unwrap();
                let slow = String::from_utf8_lossy(&request[..len]).contains("/slow");
                arrived.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                // Neither endpoint finishes until both requests reach the wire.
                barrier.wait();
                if slow { slow_wait.lock().unwrap().recv_timeout(Duration::from_secs(20)).unwrap(); }
                let body = if slow { r#"{"name":"slow"}"# } else { r#"{"name":"fast"}"# };
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }));
        }
        for worker in workers {
            worker.join().unwrap();
        }
    });
    let directory = tempfile::tempdir().unwrap();
    let store = DatabaseStore::new(directory.path().join("api-tester.sqlite3"));
    store.initialize().unwrap();
    let other = store.create_local_workspace("Other").unwrap();
    let mut app = None;
    let store_for_app = store.clone();
    let (_, cx) = cx.add_window_view(|window, cx| {
        gpui_component::init(cx);
        let bindings = shortcuts::capture_base_key_bindings(cx);
        crate::theme::configure(cx);
        let view =
            cx.new(|cx| ApiTester::new_with_database_store(bindings, store_for_app, window, cx));
        app = Some(view.clone());
        gpui_component::Root::new(view, window, cx)
    });
    let app = app.unwrap();
    let (slow_id, fast_id, source_workspace_id, environment_id) = cx.update(|window, cx| app.update(cx, |app, cx| {
        app.settings.mcp.enabled = true;
        let mut workspace = app.workspace.clone();
        let environment = workspace.create_environment("Execution environment").unwrap();
        workspace.set_active_environment(Some(&environment)).unwrap();
        let collection = workspace.create_collection("Parallel").unwrap();
        let request = workspace.create_saved_request(&collection, "Repeated",
            RequestTemplate::new(RequestDraft::new("GET", format!("http://{address}/saved")))
                .with_scripts(RequestScripts {
                    pre_request: "api.environment.set(api.request.url.endsWith('/slow') ? 'slow' : 'fast', 'pre'); console.log('pre');".to_owned(),
                    post_response: "api.environment.set(api.request.url.endsWith('/slow') ? 'slow' : 'fast', 'post'); console.log('post');".to_owned(),
                })).unwrap();
        app.commit_control_workspace(workspace, cx).unwrap();
        app.selected_environment_id = Some(environment.clone());
        app.reload_environment_editor(window, cx);
        assert!(!app.environment_editor_is_dirty(cx));
        let mut ids = Vec::new();
        for path in ["slow", "fast"] {
            let response = app.handle_window_control_call("execute_http_request",
                serde_json::json!({"request_id": request, "overrides": {"url": format!("http://{address}/{path}")}}), window, cx);
            assert!(response.ok, "{:?}", response.error);
            ids.push(response.result.unwrap()["operation_id"].as_str().unwrap().to_owned());
        }
        assert_ne!(ids[0], ids[1]);
        assert!(!app.sending, "MCP must not commandeer the visible request");
        (ids.remove(0), ids.remove(0), app.workspace_providers.active_id().clone(), environment)
    }));
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        cx.run_until_parked();
        let response = cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.handle_window_control_call(
                    "get_http_exchange",
                    serde_json::json!({"operation_id": fast_id}),
                    window,
                    cx,
                )
            })
        });
        assert!(response.ok, "{:?}", response.error);
        if response.result.unwrap()["state"] == "completed" {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "parallel HTTP requests did not complete"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(arrived.load(std::sync::atomic::Ordering::SeqCst), 2);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            assert!(
                !app.environment_editor_is_dirty(cx),
                "script mutations must refresh a previously clean environment editor"
            );
            let slow = app.handle_window_control_call(
                "get_http_exchange",
                serde_json::json!({"operation_id": slow_id}),
                window,
                cx,
            );
            assert_eq!(slow.result.unwrap()["state"], "running");
            let switched = app.handle_window_control_call(
                "switch_workspace",
                serde_json::json!({"workspace_id": format!("local:{}", other.id)}),
                window,
                cx,
            );
            assert!(switched.ok, "{:?}", switched.error);
        })
    });
    release_slow.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        cx.run_until_parked();
        let done = cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.handle_window_control_call(
                    "get_http_exchange",
                    serde_json::json!({"operation_id": slow_id}),
                    window,
                    cx,
                )
                .result
                .unwrap()["state"]
                    == "completed"
            })
        });
        if done {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    cx.update(|window, cx| app.update(cx, |app, cx| {
        for (id, name) in [(&slow_id, "slow"), (&fast_id, "fast")] {
            let result = app.handle_window_control_call("query_http_response",
                serde_json::json!({"operation_id": id, "json_pointer": "/name"}), window, cx);
            assert!(result.ok, "{:?}", result.error);
            assert_eq!(result.result.unwrap()["value"], name);
        }
        assert_eq!(app.history.len(), 2);
        assert_eq!(store.load_history().unwrap().len(), 2);
        let persisted = app.workspace_providers.provider(&source_workspace_id).unwrap().load_workspace().unwrap();
        let environment = persisted.environment(&environment_id).unwrap();
        for key in ["slow", "fast"] {
            assert_eq!(environment.variables.iter().find(|variable| variable.key == key).unwrap().value, "post");
        }
        assert!(app.workspace.environment(&environment_id).is_none(), "mutations must not leak into the newly selected workspace");

        let collection = app.workspace.create_collection("Cancellation").unwrap();
        let request = app.workspace.create_saved_request(&collection, "Infinite script",
            RequestTemplate::new(RequestDraft::new("GET", format!("http://{address}/never")))
                .with_scripts(RequestScripts {
                    pre_request: "while (true) {}".to_owned(), post_response: String::new(),
                })).unwrap();
        let mut ids = Vec::new();
        for _ in 0..2 {
            let result = app.handle_window_control_call("execute_http_request",
                serde_json::json!({"request_id": request}), window, cx);
            assert!(result.ok, "{:?}", result.error);
            ids.push(result.result.unwrap()["operation_id"].as_str().unwrap().to_owned());
        }
        let cancelled = app.handle_window_control_call("cancel_http_request",
            serde_json::json!({"operation_id": ids[0]}), window, cx);
        assert!(cancelled.ok, "{:?}", cancelled.error);
        let sibling = app.handle_window_control_call("get_http_exchange",
            serde_json::json!({"operation_id": ids[1]}), window, cx);
        assert_eq!(sibling.result.unwrap()["state"], "running");
        let first = app.handle_window_control_call("get_http_exchange",
            serde_json::json!({"operation_id": ids[0]}), window, cx);
        let first = first.result.unwrap();
        assert_eq!(first["state"], "failed");
        assert_eq!(first["error"], "Request cancelled");
        assert!(app.handle_window_control_call("cancel_http_request",
            serde_json::json!({"operation_id": ids[1]}), window, cx).ok);

        let mut background = app.workspace_providers.provider(&source_workspace_id).unwrap().load_workspace().unwrap();
        let collection = background.create_collection("Background cancellation").unwrap();
        let request = background.create_saved_request(&collection, "Background script",
            RequestTemplate::new(RequestDraft::new("GET", format!("http://{address}/never")))
                .with_scripts(RequestScripts { pre_request: "while (true) {}".to_owned(), post_response: String::new() })).unwrap();
        app.workspace_providers.provider(&source_workspace_id).unwrap().save_workspace(&background).unwrap();
        let visible = app.workspace_providers.active_id().clone();
        let result = app.handle_window_control_call("execute_http_request",
            serde_json::json!({"workspace_id": source_workspace_id.to_string(), "request_id": request}), window, cx);
        assert!(result.ok, "{:?}", result.error);
        let id = result.result.unwrap()["operation_id"].as_str().unwrap().to_owned();
        assert_eq!(app.workspace_providers.active_id(), &visible);
        assert!(app.handle_window_control_call("cancel_http_request",
            serde_json::json!({"operation_id": id}), window, cx).ok);
    }));
    server.join().unwrap();
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            let switched = app.handle_window_control_call(
                "switch_workspace",
                serde_json::json!({"workspace_id": source_workspace_id.to_string()}),
                window,
                cx,
            );
            assert!(switched.ok, "{:?}", switched.error);
            app.settings.mcp.follow_agent_activity = false;
            app.active_saved_request_id = None;
            // A console must never inherit a previous server Send's upload target.
            app.request_history_target = Some(workspace_connections::ActiveUpstreamWorkspace {
                upstream_id: "previous-server".to_owned(),
                workspace_id: "previous-workspace".to_owned(),
                base_url: "https://previous.example".parse().unwrap(),
            });
        })
    });
    for (id, expected) in [(&slow_id, "slow"), (&fast_id, "fast")] {
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                let response = app.handle_window_control_call(
                    "run_script_console",
                    serde_json::json!({
                        "http_operation_id": id,
                        "source": format!("api.assert(api.response.json().name === '{expected}');")
                    }),
                    window,
                    cx,
                );
                assert!(response.ok, "{:?}", response.error);
                assert!(app.active_saved_request_id.is_some());
                assert_eq!(app.active_saved_request_id, app.mcp_http_request_id);
                assert!(app.request_history_target.is_none());
            })
        });
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            cx.run_until_parked();
            let finished = cx.update(|_, cx| !app.read(cx).script_console_running);
            if finished {
                break;
            }
            assert!(Instant::now() < deadline, "console did not complete");
            std::thread::sleep(Duration::from_millis(5));
        }
        cx.update(|_, cx| {
            let app = app.read(cx);
            assert!(
                app.script_diagnostic.is_none(),
                "console must read the response for its execution ID"
            );
        });
    }
}

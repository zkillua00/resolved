use super::*;

#[gpui::test]
fn mcp_websocket_sessions_overlap_and_keep_independent_events(cx: &mut gpui::TestAppContext) {
    use futures::{SinkExt as _, StreamExt as _};
    use std::time::{Duration, Instant};

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind WebSocket server");
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let arrived = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let server_arrived = arrived.clone();
    let server = std::thread::spawn(move || {
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async move {
                let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                let mut tasks = Vec::new();
                for _ in 0..2 {
                    let (stream, _) = listener.accept().await.unwrap();
                    server_arrived.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    tasks.push(tokio::spawn(async move {
                        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
                        while let Some(message) = socket.next().await {
                            let message = message.expect("read WebSocket message");
                            if message.is_close() {
                                break;
                            }
                            socket.send(message).await.unwrap();
                        }
                    }));
                }
                for task in tasks {
                    let _ = task.await;
                }
            });
    });

    let directory = tempfile::tempdir().expect("create temporary control database directory");
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
    let request_id = cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.settings.mcp.enabled = true;
            let mut workspace = app.workspace.clone();
            let collection_id = workspace.create_collection("Sockets").unwrap();
            let request_id = workspace
                .create_saved_request(
                    &collection_id,
                    "Echo",
                    RequestTemplate::websocket(WebSocketWorkspace {
                        url: format!("ws://{address}/echo"),
                        ..WebSocketWorkspace::default()
                    }),
                )
                .unwrap();
            app.commit_control_workspace(workspace, cx).unwrap();
            request_id
        })
    });

    let mut connection_ids = Vec::new();
    for _ in 0..2 {
        let connected = cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.handle_window_control_call(
                    "connect_websocket",
                    serde_json::json!({ "request_id": request_id }),
                    window,
                    cx,
                )
            })
        });
        assert!(connected.ok, "{:?}", connected.error);
        connection_ids.push(connected.result.unwrap()["connection_id"].as_u64().unwrap());
    }
    assert_ne!(connection_ids[0], connection_ids[1]);

    let first_after_second = cx.update(|_, cx| {
        app.read(cx)
            .control_get_websocket_events(serde_json::json!({
                "connection_id": connection_ids[0]
            }))
            .unwrap()
    });
    assert_ne!(first_after_second["state"], "disconnected");
    assert_ne!(first_after_second["state"], "failed");
    assert!(
        !first_after_second["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["kind"] == "close")
    );

    let wait_until = |cx: &mut gpui::VisualTestContext,
                      connection_id: u64,
                      predicate: fn(&serde_json::Value) -> bool| {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            std::thread::sleep(Duration::from_millis(10));
            cx.run_until_parked();
            let events = cx.update(|_, cx| {
                app.read(cx)
                    .control_get_websocket_events(serde_json::json!({
                        "connection_id": connection_id
                    }))
                    .unwrap()
            });
            if predicate(&events) {
                return events;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for WebSocket connection {connection_id}: {events}"
            );
        }
    };
    for id in &connection_ids {
        wait_until(cx, *id, |events| events["state"] == "connected");
    }
    assert_eq!(arrived.load(std::sync::atomic::Ordering::SeqCst), 2);

    for (id, payload) in connection_ids.iter().zip(["alpha", "beta"]) {
        let sent = cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.handle_window_control_call(
                    "send_websocket_message",
                    serde_json::json!({
                        "connection_id": id,
                        "text": payload
                    }),
                    window,
                    cx,
                )
            })
        });
        assert!(sent.ok, "{:?}", sent.error);
    }
    let first = wait_until(cx, connection_ids[0], |events| {
        events["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["direction"] == "received" && event["payload"] == "alpha")
    });
    let second = wait_until(cx, connection_ids[1], |events| {
        events["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["direction"] == "received" && event["payload"] == "beta")
    });
    assert!(
        !first["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["payload"] == "beta")
    );
    assert!(
        !second["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["payload"] == "alpha")
    );

    let disconnected = cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.handle_window_control_call(
                "disconnect_websocket",
                serde_json::json!({ "connection_id": connection_ids[0] }),
                window,
                cx,
            )
        })
    });
    assert!(disconnected.ok, "{:?}", disconnected.error);
    let sibling = cx.update(|_, cx| {
        app.read(cx)
            .control_get_websocket_events(serde_json::json!({
                "connection_id": connection_ids[1]
            }))
            .unwrap()
    });
    assert_eq!(sibling["state"], "connected");
    let sent = cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.handle_window_control_call(
                "send_websocket_message",
                serde_json::json!({
                    "connection_id": connection_ids[1],
                    "text": "gamma"
                }),
                window,
                cx,
            )
        })
    });
    assert!(sent.ok, "{:?}", sent.error);
    wait_until(cx, connection_ids[1], |events| {
        events["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["direction"] == "received" && event["payload"] == "gamma")
    });
    let closed = cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.handle_window_control_call(
                "disconnect_websocket",
                serde_json::json!({ "connection_id": connection_ids[1] }),
                window,
                cx,
            )
        })
    });
    assert!(closed.ok, "{:?}", closed.error);
    server.join().unwrap();
}

//! Session-only loopback ingress. Context is captured once, never read from the
//! active tab when an exchange arrives. No editor graph or scripts are involved.
use super::*;
use gpui_component::setting::{SettingField, SettingGroup, SettingItem, SettingPage};

pub(super) struct GatewayState {
    server: Option<crate::gateway::GatewayServer>,
    generation: u64,
    starting: bool,
    pub(super) pending: usize,
    token: String,
    status: String,
}

impl Default for GatewayState {
    fn default() -> Self {
        Self {
            server: None,
            generation: 0,
            starting: false,
            pending: 0,
            token: new_token(),
            status: "Disabled — enable explicitly each session.".into(),
        }
    }
}

fn new_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

#[derive(Clone)]
struct GatewayContext {
    workspace_id: String,
    environment_id: Option<String>,
    secrets: Vec<String>,
    cookies: Arc<CookieJar>,
    prepared: execution::PreparedExecution,
}

fn history_request(request: &crate::gateway::GatewayRequest) -> Result<RequestDraft, &'static str> {
    let body =
        std::str::from_utf8(&request.body).map_err(|_| "gateway_binary_history_unsupported")?;
    let mut draft = RequestDraft::new(&request.method, &request.target);
    draft.body = body.to_owned();
    draft.body_mode = BodyMode::Raw;
    draft.headers = request
        .headers
        .iter()
        .map(|(name, value)| {
            value
                .to_str()
                .map(|value| HeaderEntry::new(name.as_str(), value))
                .map_err(|_| "gateway_header_history_unsupported")
        })
        .collect::<Result<_, _>>()?;
    Ok(draft)
}

impl ApiTester {
    pub(super) fn stop_gateway(&mut self) {
        self.gateway.generation = self.gateway.generation.wrapping_add(1);
        self.gateway.server = None;
        self.gateway.starting = false;
        self.gateway.status =
            "Disabled. Already accepted executions finish and save history.".into();
    }

    fn start_gateway(&mut self, cx: &mut Context<Self>) {
        if self.gateway.server.is_some() || self.gateway.starting {
            return;
        }
        let WorkspaceProviderId::Local(workspace_id) = self.workspace_providers.active_id() else {
            self.gateway.status = "Select a local workspace first. Remote execution is not supported in this preview.".into();
            cx.notify();
            return;
        };
        if !self.history_writable {
            self.gateway.status = "Gateway requires writable history.".into();
            cx.notify();
            return;
        }
        let workspace_id = workspace_id.clone();
        let port = match self.gateway_port_input.read(cx).value().parse::<u16>() {
            Ok(port) if port > 0 => port,
            _ => {
                self.gateway.status = "Port must be a number from 1 to 65535.".into();
                cx.notify();
                return;
            }
        };
        let mut settings = self.settings.clone();
        settings.gateway.port = port;
        if let Err(error) = self.commit_settings(settings, false, cx) {
            self.gateway.status = error;
            cx.notify();
            return;
        }
        let environment_id = self.workspace.active_environment_id.clone();
        let secrets = environment_id
            .as_deref()
            .and_then(|id| self.workspace.environment(id))
            .map(|environment| {
                environment
                    .variables
                    .iter()
                    .filter(|variable| variable.secret && variable.enabled)
                    .map(|variable| variable.value.clone())
                    .collect()
            })
            .unwrap_or_default();
        let cookies = self.cookie_jar.clone();
        let preparation = self.prepare_execution(None);
        self.gateway.generation = self.gateway.generation.wrapping_add(1);
        let generation = self.gateway.generation;
        self.gateway.starting = true;
        self.gateway.pending += 1;
        self.gateway.status = "Starting…".into();
        let token = self.gateway.token.clone();
        let (sender, mut receiver) = tokio::sync::mpsc::channel(8);
        let task = self.runtime.spawn(async move {
            let prepared = preparation.await?;
            let server = crate::gateway::GatewayServer::start(port, token, sender).await?;
            Ok::<_, String>((
                server,
                GatewayContext {
                    workspace_id,
                    environment_id,
                    secrets,
                    cookies,
                    prepared,
                },
            ))
        });
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .map_err(|error| error.to_string())
                .and_then(|result| result);
            let context = this
                .update(cx, |this, cx| {
                    this.gateway.pending -= 1;
                    if this.gateway.generation != generation {
                        return None;
                    }
                    this.gateway.starting = false;
                    match result {
                        Ok((server, context)) => {
                            this.gateway.status = format!(
                                "Listening on 127.0.0.1:{} · workspace {} · environment {}",
                                server.port(),
                                context.workspace_id,
                                context.environment_id.as_deref().unwrap_or("none")
                            );
                            this.gateway.server = Some(server);
                            cx.notify();
                            Some(context)
                        }
                        Err(error) => {
                            this.gateway.status = error;
                            cx.notify();
                            None
                        }
                    }
                })
                .ok()
                .flatten();
            let Some(context) = context else {
                return;
            };
            while let Some(exchange) = receiver.recv().await {
                let context = context.clone();
                if this
                    .update(cx, |this, cx| {
                        if this.gateway.generation != generation || this.gateway.server.is_none() {
                            let _ = exchange.reply.send(Err("Gateway was disabled.".into()));
                        } else {
                            this.accept_gateway_exchange(exchange, context, cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    fn accept_gateway_exchange(
        &mut self,
        exchange: crate::gateway::GatewayExchange,
        context: GatewayContext,
        cx: &mut Context<Self>,
    ) {
        if exchange.reply.is_closed() {
            return;
        }
        if self.gateway.pending >= 8 {
            let _ = exchange
                .reply
                .send(Ok(crate::gateway::GatewayReply::rejected(
                    429,
                    "gateway_busy",
                )));
            return;
        }
        if !self.history_writable {
            let _ = exchange
                .reply
                .send(Ok(crate::gateway::GatewayReply::rejected(
                    503,
                    "gateway_history_read_only",
                )));
            return;
        }
        // Reject before network, rather than persisting a lossy request snapshot.
        let draft = match history_request(&exchange.request) {
            Ok(draft) => draft,
            Err(code) => {
                let _ = exchange
                    .reply
                    .send(Ok(crate::gateway::GatewayReply::rejected(
                        if code == "gateway_binary_history_unsupported" {
                            415
                        } else {
                            400
                        },
                        code,
                    )));
                return;
            }
        };
        let database = self.database_store.clone();
        let workspace_id = context.workspace_id.clone();
        let environment_id = context.environment_id.clone();
        // The live workspace may contain edits not yet committed to SQLite.
        if self.workspace_providers.active_id() == &WorkspaceProviderId::Local(workspace_id.clone())
            && environment_id
                .as_deref()
                .is_some_and(|id| self.workspace.environment(id).is_none())
        {
            let _ = exchange.reply.send(Err(
                "Pinned environment was deleted. Re-enable the gateway.".into(),
            ));
            return;
        }
        self.gateway.pending += 1;
        let (request, reply) = (exchange.request, exchange.reply);
        let task = self.runtime.spawn(async move {
            let result = async {
                let validation = crate::io::run(move || {
                    let workspace = database
                        .load_workspace_for(&workspace_id)
                        .map_err(|error| error.to_string())?;
                    if environment_id
                        .as_deref()
                        .is_some_and(|id| workspace.environment(id).is_none())
                    {
                        return Err(
                            "Pinned environment was deleted. Re-enable the gateway.".to_owned()
                        );
                    }
                    Ok(())
                })
                .await
                .map_err(|error| error.to_string())?;
                validation?;
                if reply.is_closed() {
                    return Err("Gateway caller disconnected before dispatch.".to_owned());
                }
                let input = crate::core::ExecutionInput::gateway(
                    &request.method,
                    &request.target,
                    request.headers,
                    request.body.into(),
                )
                .map_err(|error| error.to_string())?;
                context
                    .prepared
                    .send_gateway_input(input, &context.cookies)
                    .await
                    .map_err(|error| error.to_string())
            }
            .await;
            (reply, result)
        });
        cx.spawn(async move |this, cx| {
            let outcome = task.await.map_err(|error| error.to_string());
            let (sender, result) = match outcome {
                Ok((reply, result)) => (Some(reply), result),
                Err(error) => (None, Err(error)),
            };
            let entry = match &result {
                Ok(response) => HistoryEntry::completed_with_secrets(&draft, response, &context.secrets),
                Err(error) => HistoryEntry::failed_with_secrets(&draft, error, &context.secrets),
            };
            if this.update(cx, |this, cx| {
                this.history.push(entry);
                this.persist_execution_history(cx);
                cx.notify();
            }).is_err() { return; }
            // Do not advertise queued persistence as success. FIFO owner saves also
            // coordinate with Clear History and other concurrent executions.
            let saved = loop {
                match this.read_with(cx, |this, _| {
                    if this.persistence_io.history_pending || this.persistence_io.history_clear_pending {
                        None
                    } else {
                        Some(!this.persistence_io.history_failed && this.history_writable)
                    }
                }) {
                    Ok(Some(saved)) => break saved,
                    Err(_) => break false,
                    _ => {
                        Timer::after(Duration::from_millis(10)).await;
                    }
                }
            };
            let reply = if saved {
                result.map_err(|_| "Gateway execution failed. See Resolved history for details; do not retry automatically.".to_owned()).map(|response| crate::gateway::GatewayReply {
                    status: response.status,
                    headers: response.headers.into_iter().map(|header| (header.name, header.value.into_bytes())).collect(),
                    body: response.body,
                })
            } else {
                Err("Execution finished, but history could not be persisted. Do not retry automatically.".into())
            };
            if let Some(sender) = sender {
                let _ = sender.send(reply);
            }
            let _ = this.update(cx, |this, cx| { this.gateway.pending -= 1; cx.notify(); });
        }).detach();
    }

    pub(super) fn gateway_settings_page(&self, cx: &mut Context<Self>) -> SettingPage {
        let this = cx.entity().downgrade();
        SettingPage::new("Gateway")
            .description("Local loopback preview. Explicit enable required after every launch.")
            .resettable(false)
            .group(SettingGroup::new().title("Session")
                .description("Use current workspace/environment: captured at enable, including cookies and execution limits. Switching tabs never retargets calls. Disable and re-enable to refresh. Local workspaces only; literal UTF-8 requests, no interpolation, scripts, DNS, browser Origin, or per-request selectors. History stores a redacted request and response summary, not a byte-exact replay.")
                .item(SettingItem::new("Gateway controls", SettingField::<SharedString>::render(move |_, _, cx| {
                    let Some(entity) = this.upgrade() else { return div().into_any_element(); };
                    let state = entity.read(cx);
                    let enabled = state.gateway.server.is_some() || state.gateway.starting;
                    let status = state.gateway.status.clone();
                    let port = state.settings.gateway.port;
                    let port_input = state.gateway_port_input.clone();
                    let enable = this.clone();
                    let copy = this.clone();
                    let rotate = this.clone();
                    v_flex().gap_2().child(status)
                        .child(h_flex().gap_2().child("Loopback port").child(Input::new(&port_input).disabled(enabled)))
                        .child(h_flex().flex_wrap().gap_2()
                        .child(Button::new("gateway-enable").label(if enabled { "Disable" } else { "Use current workspace/environment & enable" })
                            .on_click(move |_, _, cx| { if let Some(entity) = enable.upgrade() {
                                entity.update(cx, |this, cx| { if enabled { this.stop_gateway(); cx.notify(); } else { this.start_gateway(cx); } });
                            }}))
                        .child(Button::new("gateway-copy-token").label("Copy token").on_click(move |_, _, cx| {
                            if let Some(entity) = copy.upgrade() { cx.write_to_clipboard(ClipboardItem::new_string(entity.read(cx).gateway.token.clone())); }
                        }))
                        .child(Button::new("gateway-rotate").label("Rotate token & disable").on_click(move |_, _, cx| {
                            if let Some(entity) = rotate.upgrade() { entity.update(cx, |this, cx| {
                                this.stop_gateway(); this.gateway.token = new_token(); cx.notify();
                            }); }
                        }))
                        .child(Button::new("gateway-example").label("Copy curl example").on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(format!(
                                "curl -H 'X-Resolved-Gateway-Token: PASTE_SESSION_TOKEN' -H 'X-Resolved-Target: https://example.com' http://127.0.0.1:{port}/"
                            )));
                        }))).into_any_element()
                }))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn loopback_gateway_keeps_pinned_environment_and_persists_history(
        cx: &mut gpui::TestAppContext,
    ) {
        use std::io::{Read as _, Write as _};
        use std::net::{TcpListener, TcpStream};
        use std::time::Instant;

        let upstream = TcpListener::bind("127.0.0.1:0").unwrap();
        let upstream_address = upstream.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = upstream.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let mut request = Vec::new();
            let mut byte = [0];
            while !request.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
            }
            assert!(request.starts_with(b"GET /a%2Fb?x=1&x=&q=a+b&z=%20 HTTP/1.1\r\n"));
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .unwrap();
        });
        let port_reservation = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = port_reservation.local_addr().unwrap().port();
        drop(port_reservation);
        let directory = tempfile::tempdir().unwrap();
        let store = DatabaseStore::new(directory.path().join("gateway.sqlite3"));
        store.initialize().unwrap();
        let other = store.create_local_workspace("Other").unwrap();
        let store_for_app = store.clone();
        let mut app = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            gpui_component::init(cx);
            let bindings = shortcuts::capture_base_key_bindings(cx);
            crate::theme::configure(cx);
            let view = cx
                .new(|cx| ApiTester::new_with_database_store(bindings, store_for_app, window, cx));
            app = Some(view.clone());
            gpui_component::Root::new(view, window, cx)
        });
        let app = app.unwrap();
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                let mut workspace = app.workspace.clone();
                let id = workspace.create_environment("Pinned").unwrap();
                let editing = workspace.create_environment("Editing, not active").unwrap();
                workspace.active_environment_id = Some(id.clone());
                app.commit_control_workspace(workspace, cx).unwrap();
                app.selected_environment_id = Some(editing);
                app.gateway_port_input.update(cx, |input, cx| {
                    input.set_value(port.to_string(), window, cx)
                });
                app.start_gateway(cx);
            })
        });
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            cx.run_until_parked();
            if cx.update(|_, cx| app.read(cx).gateway.server.is_some()) {
                break;
            }
            assert!(Instant::now() < deadline, "gateway did not start");
            std::thread::sleep(Duration::from_millis(5));
        }
        let token = cx.update(|_, cx| {
            app.update(cx, |app, _| {
                assert!(
                    app.gateway
                        .status
                        .contains(app.workspace.active_environment_id.as_deref().unwrap())
                );
                // Simulate selecting another persisted workspace after enable.
                app.workspace_providers.register(Arc::new(
                    crate::core::LocalWorkspaceProvider::new(store.clone(), other.id.clone()),
                ));
                app.workspace_providers
                    .switch(WorkspaceProviderId::Local(other.id.clone()))
                    .unwrap();
                app.replace_workspace(store.load_workspace_for(&other.id).unwrap());
                app.selected_environment_id = None;
                app.gateway.token.clone()
            })
        });
        let (sent, received) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut client = TcpStream::connect(("127.0.0.1", port)).unwrap();
            client
                .set_read_timeout(Some(Duration::from_secs(20)))
                .unwrap();
            write!(client, "GET /a%2Fb?x=1&x=&q=a+b&z=%20 HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Resolved-Target: http://{upstream_address}\r\nX-Resolved-Gateway-Token: {token}\r\nConnection: close\r\n\r\n").unwrap();
            let mut response = String::new();
            client.read_to_string(&mut response).unwrap();
            sent.send(response).unwrap();
        });
        let response = loop {
            cx.run_until_parked();
            if let Ok(response) = received.try_recv() {
                break response;
            }
            assert!(Instant::now() < deadline, "gateway exchange did not finish");
            std::thread::sleep(Duration::from_millis(5));
        };
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(response.ends_with("ok"));
        server.join().unwrap();
        let history = store.load_history().unwrap();
        assert_eq!(
            history.len(),
            1,
            "reply must wait for durable owner history"
        );
        assert_eq!(
            history.entries()[0].request.url,
            format!("http://{upstream_address}/a%2Fb?x=1&x=&q=a+b&z=%20")
        );
        cx.update(|_, cx| {
            app.update(cx, |app, _| {
                assert!(!app.sending, "gateway must not commandeer the editor");
                assert_eq!(app.history.len(), 1);
                app.stop_gateway();
            })
        });
    }

    #[gpui::test]
    fn gateway_timeout_finalizes_history_and_releases_pending_after_disconnect(
        cx: &mut gpui::TestAppContext,
    ) {
        use std::io::Read as _;
        use std::net::TcpListener;
        use std::time::Instant;
        let upstream = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = upstream.local_addr().unwrap();
        let (accepted_tx, accepted_rx) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = upstream.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let mut byte = [0];
            while !request.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
            }
            accepted_tx.send(()).unwrap();
            // Send no response; the selected execution deadline must close it.
            assert_eq!(stream.read(&mut byte).unwrap(), 0);
        });
        let directory = tempfile::tempdir().unwrap();
        let store = DatabaseStore::new(directory.path().join("timeout.sqlite3"));
        let app_store = store.clone();
        let mut app = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            gpui_component::init(cx);
            let bindings = shortcuts::capture_base_key_bindings(cx);
            crate::theme::configure(cx);
            let view =
                cx.new(|cx| ApiTester::new_with_database_store(bindings, app_store, window, cx));
            app = Some(view.clone());
            gpui_component::Root::new(view, window, cx)
        });
        let app = app.unwrap();
        let (reply, receiver) = tokio::sync::oneshot::channel();
        cx.update(|_, cx| {
            app.update(cx, |app, cx| {
                let WorkspaceProviderId::Local(workspace_id) =
                    app.workspace_providers.active_id().clone()
                else {
                    panic!("local workspace");
                };
                let mut limits = crate::core::execution_limits::ExecutionLimits::default();
                limits.0.insert(
                    "http.timeout_ms".into(),
                    crate::core::execution_limits::Bound::limited(200),
                );
                app.accept_gateway_exchange(
                    crate::gateway::GatewayExchange {
                        request: crate::gateway::GatewayRequest {
                            method: "GET".into(),
                            target: format!("http://{address}/stall"),
                            headers: reqwest::header::HeaderMap::new(),
                            body: bytes::Bytes::new(),
                        },
                        reply,
                    },
                    GatewayContext {
                        workspace_id,
                        environment_id: None,
                        secrets: Vec::new(),
                        cookies: app.cookie_jar.clone(),
                        prepared: execution::PreparedExecution {
                            scope_key: "local:timeout-test".into(),
                            limits,
                            upstream: None,
                        },
                    },
                    cx,
                );
            })
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            cx.run_until_parked();
            if accepted_rx.try_recv().is_ok() {
                break;
            }
            assert!(Instant::now() < deadline, "request did not reach upstream");
            std::thread::sleep(Duration::from_millis(2));
        }
        drop(receiver);
        cx.update(|_, cx| app.update(cx, |app, _| app.stop_gateway()));
        loop {
            cx.run_until_parked();
            if cx.update(|_, cx| app.read(cx).gateway.pending == 0) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "gateway did not release pending execution"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        server.join().unwrap();
        let history = store.load_history().unwrap();
        assert_eq!(history.len(), 1);
        assert!(
            history.entries()[0]
                .error
                .as_deref()
                .unwrap()
                .contains("http.timeout_ms")
        );
    }

    #[test]
    fn binary_history_is_rejected_and_literal_query_is_preserved() {
        let mut request = crate::gateway::GatewayRequest {
            method: "mIxEd".into(),
            target: "https://example.com/a%2Fb?x=1&x=&q=a+b&z=%20".into(),
            headers: reqwest::header::HeaderMap::new(),
            body: bytes::Bytes::from_static(b"\xff"),
        };
        assert!(history_request(&request).is_err());
        request.body = bytes::Bytes::from_static(b"{\"hello\":true}");
        let draft = history_request(&request).unwrap();
        assert_eq!(draft.url, request.target);
        assert_eq!(draft.method, request.method);
        assert_eq!(draft.body.as_bytes(), request.body.as_ref());
    }

    #[test]
    fn sessions_start_disabled_with_distinct_tokens() {
        let first = GatewayState::default();
        let second = GatewayState::default();
        assert!(first.server.is_none());
        assert!(!first.starting);
        assert_ne!(first.token, second.token);
        assert_eq!(first.token.len(), 64);
    }
}

use super::*;
use crate::control_server::{ControlCall, ControlResponse};
use base64::Engine as _;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedReceiver;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ControlWebSocketStatus {
    Connecting,
    Connected,
    Closing,
    Disconnected,
    Failed,
}

impl ControlWebSocketStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Connecting => "connecting",
            Self::Connected => "connected",
            Self::Closing => "closing",
            Self::Disconnected => "disconnected",
            Self::Failed => "failed",
        }
    }
}

#[derive(Clone, Debug)]
struct ControlWebSocketEvent {
    id: u64,
    at: chrono::DateTime<Utc>,
    direction: &'static str,
    kind: &'static str,
    payload: Option<String>,
    binary: Option<Vec<u8>>,
}

pub(super) struct ControlWebSocketConnection {
    id: u64,
    request_id: String,
    status: ControlWebSocketStatus,
    notice: Option<String>,
    sender: tokio::sync::mpsc::UnboundedSender<WebSocketCommand>,
    event_sender: tokio::sync::mpsc::UnboundedSender<ControlWebSocketIncoming>,
    abort_handle: AbortHandle,
    events: Vec<ControlWebSocketEvent>,
    next_event_id: u64,
}

enum ControlWebSocketIncoming {
    Wire(WebSocketSignal),
    SentText(String),
    ScriptLog(String),
    ScriptError(String),
}

#[derive(Clone)]
pub(super) struct McpHttpExchangeSnapshot {
    operation_id: u64,
    state: &'static str,
    stage: Option<&'static str>,
    request: Option<RequestDraft>,
    response: Option<ResponseData>,
    error: Option<String>,
    diagnostic: Option<ScriptDiagnostic>,
    pre_request_report: Option<ScriptReport>,
    post_response_report: Option<ScriptReport>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateRequestParams {
    collection_id: String,
    #[serde(default)]
    folder_id: Option<String>,
    name: String,
    #[serde(default)]
    request: Option<RequestDraft>,
    #[serde(default)]
    websocket: Option<WebSocketWorkspace>,
    #[serde(default)]
    scripts: RequestScripts,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SaveRequestParams {
    request_id: String,
    expected_updated_at: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    request: Option<RequestDraft>,
    #[serde(default)]
    websocket: Option<WebSocketWorkspace>,
    #[serde(default)]
    clear_websocket: bool,
    #[serde(default)]
    scripts: Option<RequestScripts>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetScriptsParams {
    request_id: String,
    expected_updated_at: String,
    pre_request: String,
    post_response: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetVariableParams {
    environment_id: String,
    #[serde(default)]
    variable_id: Option<String>,
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    secret: Option<bool>,
}

enum ControlDispatch {
    Immediate(ControlResponse),
    Remote(RemoteControlPending),
}

struct RemoteControlPending {
    target: ActiveUpstreamWorkspace,
    generation: u64,
    task: tokio::task::JoinHandle<Result<RemoteControlOutcome, String>>,
}

enum RemoteControlMutation {
    CreateCollection {
        name: String,
    },
    CreateRequest {
        target_collection_id: String,
        name: String,
        definition: RequestTemplate,
    },
    SaveRequest {
        target_collection_id: String,
        request_id: String,
        expected_updated_at: String,
        name: String,
        definition: RequestTemplate,
    },
    CreateEnvironment {
        name: String,
    },
    RenameEnvironment {
        environment_id: String,
        name: String,
    },
    CreateEnvironmentVariable {
        environment_id: String,
        variable: EnvironmentVariable,
    },
    UpdateEnvironmentVariable {
        environment_id: String,
        baseline: EnvironmentVariable,
        draft: EnvironmentVariable,
    },
}

enum RemoteControlSnapshot {
    Workspace(UpstreamWorkspaceView),
    Environments(Vec<UpstreamEnvironmentView>),
}

enum RemoteControlResult {
    Collection(String),
    Request(String),
    Environment(String),
    Variable {
        environment_id: String,
        variable_id: String,
    },
}

struct RemoteControlOutcome {
    snapshot: RemoteControlSnapshot,
    result: RemoteControlResult,
}

impl RemoteControlMutation {
    async fn execute(
        self,
        client: &Client,
        target: &ActiveUpstreamWorkspace,
        bearer_token: &str,
    ) -> Result<RemoteControlOutcome, String> {
        match self {
            Self::CreateCollection { name } => {
                let created = create_upstream_collection(
                    client,
                    &target.base_url,
                    bearer_token,
                    &target.workspace_id,
                    &name,
                    None,
                )
                .await
                .map_err(|error| error.to_string())?;
                let workspace = reload_remote_workspace(client, target, bearer_token).await?;
                Ok(RemoteControlOutcome {
                    snapshot: RemoteControlSnapshot::Workspace(workspace),
                    result: RemoteControlResult::Collection(created.id),
                })
            }
            Self::CreateRequest {
                target_collection_id,
                name,
                definition,
            } => {
                let created = create_upstream_saved_request(
                    client,
                    &target.base_url,
                    bearer_token,
                    &target.workspace_id,
                    &target_collection_id,
                    &name,
                    &definition,
                )
                .await
                .map_err(|error| error.to_string())?;
                let workspace = reload_remote_workspace(client, target, bearer_token).await?;
                Ok(RemoteControlOutcome {
                    snapshot: RemoteControlSnapshot::Workspace(workspace),
                    result: RemoteControlResult::Request(created.id),
                })
            }
            Self::SaveRequest {
                target_collection_id,
                request_id,
                expected_updated_at,
                name,
                definition,
            } => {
                let current = reload_remote_workspace(client, target, bearer_token).await?;
                let current_workspace = current.clone().into_local_workspace();
                let (_, current_request) = current_workspace
                    .saved_request(&request_id)
                    .ok_or_else(|| format!("request '{request_id}' was not found"))?;
                ensure_request_revision(current_request, &expected_updated_at)?;
                let saved = update_upstream_saved_request(
                    client,
                    &target.base_url,
                    bearer_token,
                    &target.workspace_id,
                    &target_collection_id,
                    &request_id,
                    &name,
                    &definition,
                )
                .await
                .map_err(|error| error.to_string())?;
                let workspace = reload_remote_workspace(client, target, bearer_token).await?;
                Ok(RemoteControlOutcome {
                    snapshot: RemoteControlSnapshot::Workspace(workspace),
                    result: RemoteControlResult::Request(saved.id),
                })
            }
            Self::CreateEnvironment { name } => {
                let created = create_upstream_environment(
                    client,
                    &target.base_url,
                    bearer_token,
                    &target.workspace_id,
                    &name,
                )
                .await
                .map_err(|error| error.to_string())?;
                let environments = reload_remote_environments(client, target, bearer_token).await?;
                Ok(RemoteControlOutcome {
                    snapshot: RemoteControlSnapshot::Environments(environments),
                    result: RemoteControlResult::Environment(created.id),
                })
            }
            Self::RenameEnvironment {
                environment_id,
                name,
            } => {
                let updated = update_upstream_environment(
                    client,
                    &target.base_url,
                    bearer_token,
                    &target.workspace_id,
                    &environment_id,
                    &name,
                )
                .await
                .map_err(|error| error.to_string())?;
                let environments = reload_remote_environments(client, target, bearer_token).await?;
                Ok(RemoteControlOutcome {
                    snapshot: RemoteControlSnapshot::Environments(environments),
                    result: RemoteControlResult::Environment(updated.id),
                })
            }
            Self::CreateEnvironmentVariable {
                environment_id,
                variable,
            } => {
                let created = create_upstream_environment_variable(
                    client,
                    &target.base_url,
                    bearer_token,
                    &target.workspace_id,
                    &environment_id,
                    &variable,
                )
                .await
                .map_err(|error| error.to_string())?;
                let environments = reload_remote_environments(client, target, bearer_token).await?;
                Ok(RemoteControlOutcome {
                    snapshot: RemoteControlSnapshot::Environments(environments),
                    result: RemoteControlResult::Variable {
                        environment_id,
                        variable_id: created.id,
                    },
                })
            }
            Self::UpdateEnvironmentVariable {
                environment_id,
                baseline,
                draft,
            } => {
                if baseline.key != draft.key
                    || baseline.enabled != draft.enabled
                    || baseline.secret != draft.secret
                {
                    update_upstream_environment_variable(
                        client,
                        &target.base_url,
                        bearer_token,
                        &target.workspace_id,
                        &environment_id,
                        &draft.id,
                        &draft,
                    )
                    .await
                    .map_err(|error| error.to_string())?;
                }
                if baseline.value != draft.value {
                    put_upstream_environment_variable_value(
                        client,
                        &target.base_url,
                        bearer_token,
                        &target.workspace_id,
                        &environment_id,
                        &draft.id,
                        &draft.value,
                    )
                    .await
                    .map_err(|error| error.to_string())?;
                }
                let variable_id = draft.id;
                let environments = reload_remote_environments(client, target, bearer_token).await?;
                Ok(RemoteControlOutcome {
                    snapshot: RemoteControlSnapshot::Environments(environments),
                    result: RemoteControlResult::Variable {
                        environment_id,
                        variable_id,
                    },
                })
            }
        }
    }
}

async fn reload_remote_workspace(
    client: &Client,
    target: &ActiveUpstreamWorkspace,
    bearer_token: &str,
) -> Result<UpstreamWorkspaceView, String> {
    get_upstream_workspace(client, &target.base_url, bearer_token, &target.workspace_id)
        .await
        .map_err(|error| format!("The workspace could not be refreshed: {error}"))
}

async fn reload_remote_environments(
    client: &Client,
    target: &ActiveUpstreamWorkspace,
    bearer_token: &str,
) -> Result<Vec<UpstreamEnvironmentView>, String> {
    list_upstream_environments(client, &target.base_url, bearer_token, &target.workspace_id)
        .await
        .map_err(|error| format!("The environments could not be refreshed: {error}"))
}

impl ApiTester {
    pub(super) fn capture_mcp_http_exchange(&mut self, operation_id: u64) {
        if self.mcp_http_operation_id != Some(operation_id) {
            return;
        }
        self.mcp_http_exchange = Some(McpHttpExchangeSnapshot {
            operation_id,
            state: if self.sending {
                "running"
            } else if self.response.is_some() {
                "completed"
            } else if self.request_error.is_some() {
                "failed"
            } else {
                "idle"
            },
            stage: self.execution_stage.map(ExecutionStage::label),
            request: self.response_request.clone(),
            response: self.response.clone(),
            error: self.request_error.clone(),
            diagnostic: self.script_diagnostic.clone(),
            pre_request_report: self.pre_script_report.clone(),
            post_response_report: self.post_script_report.clone(),
        });
    }

    pub(super) fn sync_control_server(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        let data_directory = self
            .database_store
            .path()
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .to_path_buf();
        if !self.settings.mcp.enabled {
            self.stop_mcp_runtime_operations(cx);
            self._control_server = None;
            crate::control_server::ControlServer::remove_stale_files(&data_directory)
                .map_err(|error| error.to_string())?;
            return Ok(());
        }
        if self._control_server.is_some() {
            return Ok(());
        }

        let (server, receiver) = crate::control_server::ControlServer::start(&data_directory)
            .map_err(|error| error.to_string())?;
        self._control_server = Some(server);
        self.attach_control_plane(receiver, cx);
        Ok(())
    }

    pub(super) fn stop_mcp_websocket(&mut self) {
        if let Some(connection) = self.mcp_websocket.take() {
            let _ = connection.sender.send(WebSocketCommand::Close);
            connection.abort_handle.abort();
        }
    }

    pub(super) fn stop_mcp_runtime_operations(&mut self, cx: &mut Context<Self>) {
        self.stop_mcp_websocket();
        self.stop_mcp_script_console();
        self.stop_mcp_http_request(cx);
    }

    pub(super) fn stop_mcp_script_console(&mut self) {
        if self.script_console_running
            && self.mcp_script_console_owned
            && let Some(cancellation) = self.script_cancellation.as_ref()
        {
            cancellation.cancel();
        }
    }

    pub(super) fn stop_mcp_http_request(&mut self, cx: &mut Context<Self>) {
        if self.sending && self.mcp_http_operation_id == Some(self.request_generation) {
            self.cancel_request(cx);
        }
    }

    pub(super) fn attach_control_plane(
        &mut self,
        mut receiver: UnboundedReceiver<ControlCall>,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |weak_this, cx| {
            while let Some(call) = receiver.recv().await {
                let Some(this) = weak_this.upgrade() else {
                    call.respond(ControlResponse::error("Resolved is shutting down"));
                    break;
                };
                let dispatch = if matches!(
                    call.method.as_str(),
                    "execute_http_request" | "run_script_console"
                ) {
                    let result = cx.update(|cx| {
                        let window_handle = cx
                            .active_window()
                            .or_else(|| cx.windows().first().copied())
                            .ok_or_else(|| "Resolved has no open window".to_owned())?;
                        window_handle
                            .update(cx, |_, window, cx| {
                                this.update(cx, |this, cx| {
                                    this.handle_window_control_call(
                                        &call.method,
                                        call.params.clone(),
                                        window,
                                        cx,
                                    )
                                })
                            })
                            .map_err(|error| error.to_string())
                    });
                    let response = match result {
                        Ok(Ok(response)) => response,
                        Ok(Err(error)) => ControlResponse::error(error),
                        Err(error) => ControlResponse::error(error.to_string()),
                    };
                    ControlDispatch::Immediate(response)
                } else {
                    this.update(cx, |this, cx| {
                        this.dispatch_control_call(&call.method, call.params.clone(), cx)
                    })
                    .unwrap_or_else(|error| {
                        ControlDispatch::Immediate(ControlResponse::error(error.to_string()))
                    })
                };
                let response = match dispatch {
                    ControlDispatch::Immediate(response) => response,
                    ControlDispatch::Remote(pending) => {
                        let target = pending.target.clone();
                        let generation = pending.generation;
                        let result = pending.task.await;
                        let Some(this) = weak_this.upgrade() else {
                            call.respond(ControlResponse::error("Resolved is shutting down"));
                            break;
                        };
                        this.update(cx, |this, cx| {
                            this.finish_remote_control_call(target, generation, result, cx)
                        })
                        .unwrap_or_else(|error| ControlResponse::error(error.to_string()))
                    }
                };
                call.respond(response);
            }
        })
        .detach();
    }

    fn dispatch_control_call(
        &mut self,
        method: &str,
        params: Value,
        cx: &mut Context<Self>,
    ) -> ControlDispatch {
        if method == "__list_enabled_tools"
            || !matches!(
                self.workspace_providers.active_id(),
                WorkspaceProviderId::Upstream { .. }
            )
            || !is_mutating_control_method(method)
            || method == "set_active_environment"
            || matches!(
                method,
                "cancel_http_request"
                    | "connect_websocket"
                    | "send_websocket_message"
                    | "run_websocket_replay"
                    | "disconnect_websocket"
            )
        {
            return ControlDispatch::Immediate(self.handle_control_call(method, params, cx));
        }
        if let Some(response) = self.validate_control_method(method) {
            return ControlDispatch::Immediate(response);
        }
        match self.prepare_remote_control_call(method, params, cx) {
            Ok(pending) => ControlDispatch::Remote(pending),
            Err(error) => ControlDispatch::Immediate(ControlResponse::error(error)),
        }
    }

    fn handle_window_control_call(
        &mut self,
        method: &str,
        params: Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ControlResponse {
        if let Some(response) = self.validate_control_method(method) {
            return response;
        }
        let result = match method {
            "execute_http_request" => {
                required_string(&params, "request_id").and_then(|request_id| {
                    self.start_control_http_request(&request_id, window, cx)
                    .map(|operation_id| json!({ "operation_id": operation_id, "state": "running" }))
                })
            }
            "run_script_console" => required_string(&params, "source").and_then(|source| {
                self.start_control_script_console(source, window, cx)
                    .map(|operation_id| json!({ "operation_id": operation_id, "state": "running" }))
            }),
            _ => Err(format!("unknown window control method '{method}'")),
        };
        match result {
            Ok(result) => ControlResponse::success(result),
            Err(error) => ControlResponse::error(error),
        }
    }

    fn validate_control_method(&self, method: &str) -> Option<ControlResponse> {
        if !self.settings.mcp.enabled {
            return Some(ControlResponse::error(
                "MCP is disabled in Resolved settings",
            ));
        }
        if crate::control_tools::tool(method).is_none() {
            return Some(ControlResponse::error(format!(
                "unknown local control method '{method}'"
            )));
        }
        if !self.settings.mcp.tool_enabled(method) {
            return Some(ControlResponse::error(format!(
                "the MCP tool '{method}' is disabled in Resolved settings"
            )));
        }
        if !self.control_tool_available_in_active_workspace(method) {
            return Some(ControlResponse::error(
                "MCP access to server workspaces is disabled in Resolved settings",
            ));
        }
        None
    }

    fn control_tool_available_in_active_workspace(&self, method: &str) -> bool {
        !matches!(
            self.workspace_providers.active_id(),
            WorkspaceProviderId::Upstream { .. }
        ) || self.settings.mcp.allow_remote_workspaces
            || matches!(method, "status" | "list_workspaces")
    }

    fn control_tool_advertised(&self, method: &str) -> bool {
        self.settings.mcp.tool_enabled(method)
            && self.control_tool_available_in_active_workspace(method)
    }

    fn prepare_remote_control_call(
        &mut self,
        method: &str,
        params: Value,
        cx: &mut Context<Self>,
    ) -> Result<RemoteControlPending, String> {
        if self.sending || self.workspace_switch_status.busy() {
            return Err("The active workspace is busy; retry the MCP call shortly.".to_owned());
        }
        let target = self.active_upstream_workspace()?;
        let (mutation, permissions) = self.remote_control_mutation(method, params)?;
        for permission in permissions {
            if !self.active_upstream_has_permission(permission) {
                return Err(format!(
                    "The signed-in server user lacks the '{permission}' permission required by '{method}'."
                ));
            }
        }

        self.workspace_switch_generation = self.workspace_switch_generation.wrapping_add(1);
        let generation = self.workspace_switch_generation;
        self.workspace_switch_status = WorkspaceSwitchStatus::Loading;
        let vault = self.credential_vault.clone();
        let client = self.upstream_client.clone();
        let runtime = Arc::clone(&self.runtime);
        let credential_upstream_id = target.upstream_id.clone();
        let task_target = target.clone();
        let task = self.runtime.spawn(async move {
            let credential = runtime
                .spawn_blocking(move || vault.load_upstream(&credential_upstream_id))
                .await
                .map_err(|error| format!("Could not open the saved session: {error}"))?
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "Log in to this server again.".to_owned())?;
            if credential.expires_at <= Utc::now() {
                return Err("Log in to this server again.".to_owned());
            }
            mutation
                .execute(&client, &task_target, credential.bearer_token())
                .await
        });
        self.workspace_switch_abort_handle = Some(task.abort_handle());
        cx.notify();
        Ok(RemoteControlPending {
            target,
            generation,
            task,
        })
    }

    fn remote_control_mutation(
        &self,
        method: &str,
        params: Value,
    ) -> Result<(RemoteControlMutation, Vec<&'static str>), String> {
        match method {
            "create_collection" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Params {
                    name: String,
                }
                let params: Params = decode(params)?;
                Ok((
                    RemoteControlMutation::CreateCollection { name: params.name },
                    vec![WORKSPACES_READ, COLLECTIONS_CREATE],
                ))
            }
            "create_request" => {
                let params: CreateRequestParams = decode(params)?;
                let collection = self
                    .workspace
                    .collection(&params.collection_id)
                    .ok_or_else(|| {
                        format!("collection '{}' was not found", params.collection_id)
                    })?;
                if let Some(folder_id) = params.folder_id.as_deref()
                    && collection.folder(folder_id).is_none()
                {
                    return Err(format!("folder '{folder_id}' was not found"));
                }
                let target_collection_id = params
                    .folder_id
                    .unwrap_or_else(|| params.collection_id.clone());
                let definition =
                    request_definition(params.request, params.websocket, params.scripts)?;
                Ok((
                    RemoteControlMutation::CreateRequest {
                        target_collection_id,
                        name: params.name,
                        definition,
                    },
                    vec![WORKSPACES_READ, REQUESTS_CREATE],
                ))
            }
            "save_request" => {
                let params: SaveRequestParams = decode(params)?;
                self.remote_save_request_mutation(params)
            }
            "set_request_scripts" => {
                let params: SetScriptsParams = decode(params)?;
                self.remote_save_request_mutation(SaveRequestParams {
                    request_id: params.request_id,
                    expected_updated_at: params.expected_updated_at,
                    name: None,
                    request: None,
                    websocket: None,
                    clear_websocket: false,
                    scripts: Some(RequestScripts {
                        pre_request: params.pre_request,
                        post_response: params.post_response,
                    }),
                })
            }
            "create_environment" => Ok((
                RemoteControlMutation::CreateEnvironment {
                    name: required_string(&params, "name")?,
                },
                vec![ENVIRONMENTS_READ, ENVIRONMENTS_CREATE],
            )),
            "rename_environment" => {
                let environment_id = required_string(&params, "environment_id")?;
                if self.workspace.environment(&environment_id).is_none() {
                    return Err(format!("environment '{environment_id}' was not found"));
                }
                Ok((
                    RemoteControlMutation::RenameEnvironment {
                        environment_id,
                        name: required_string(&params, "name")?,
                    },
                    vec![ENVIRONMENTS_READ, ENVIRONMENTS_UPDATE],
                ))
            }
            "set_environment_variable" => self.remote_environment_variable_mutation(params),
            _ => Err(format!(
                "the MCP tool '{method}' does not support remote workspaces"
            )),
        }
    }

    fn remote_save_request_mutation(
        &self,
        params: SaveRequestParams,
    ) -> Result<(RemoteControlMutation, Vec<&'static str>), String> {
        let (collection, existing) = self
            .workspace
            .saved_request(&params.request_id)
            .ok_or_else(|| format!("request '{}' was not found", params.request_id))?;
        ensure_request_revision(existing, &params.expected_updated_at)?;
        let mut definition = existing.definition.clone();
        if let Some(request) = params.request {
            definition.request = request;
            definition.websocket = None;
        }
        if let Some(websocket) = params.websocket {
            definition.request = RequestDraft::default();
            definition.websocket = Some(websocket);
        } else if params.clear_websocket {
            definition.websocket = None;
        }
        if let Some(scripts) = params.scripts {
            definition.scripts = scripts;
        }
        Ok((
            RemoteControlMutation::SaveRequest {
                target_collection_id: existing
                    .folder_id
                    .clone()
                    .unwrap_or_else(|| collection.id.clone()),
                request_id: params.request_id,
                expected_updated_at: params.expected_updated_at,
                name: params.name.unwrap_or_else(|| existing.name.clone()),
                definition,
            },
            vec![WORKSPACES_READ, REQUESTS_UPDATE],
        ))
    }

    fn remote_environment_variable_mutation(
        &self,
        params: Value,
    ) -> Result<(RemoteControlMutation, Vec<&'static str>), String> {
        let params: SetVariableParams = decode(params)?;
        let environment = self
            .workspace
            .environment(&params.environment_id)
            .ok_or_else(|| format!("environment '{}' was not found", params.environment_id))?;
        if let Some(variable_id) = params.variable_id {
            let baseline = environment
                .variables
                .iter()
                .find(|variable| variable.id == variable_id)
                .cloned()
                .ok_or_else(|| format!("variable '{variable_id}' was not found"))?;
            if baseline.secret && params.secret == Some(false) {
                return Err(
                    "secret variables cannot be made non-secret through MCP; use the Resolved UI"
                        .to_owned(),
                );
            }
            let draft = EnvironmentVariable {
                id: baseline.id.clone(),
                key: params.key.unwrap_or_else(|| baseline.key.clone()),
                value: params.value.unwrap_or_else(|| baseline.value.clone()),
                enabled: params.enabled.unwrap_or(baseline.enabled),
                secret: params.secret.unwrap_or(baseline.secret),
                created_by: baseline.created_by.clone(),
            };
            let mut permissions = vec![ENVIRONMENTS_READ];
            if baseline.key != draft.key
                || baseline.enabled != draft.enabled
                || baseline.secret != draft.secret
            {
                permissions.push(ENVIRONMENTS_UPDATE);
            }
            if baseline.value != draft.value {
                permissions.push(ENVIRONMENT_VALUES_UPDATE);
            }
            Ok((
                RemoteControlMutation::UpdateEnvironmentVariable {
                    environment_id: params.environment_id,
                    baseline,
                    draft,
                },
                permissions,
            ))
        } else {
            let key = params
                .key
                .ok_or_else(|| "key is required when creating a variable".to_owned())?;
            let value = params
                .value
                .ok_or_else(|| "value is required when creating a variable".to_owned())?;
            Ok((
                RemoteControlMutation::CreateEnvironmentVariable {
                    environment_id: params.environment_id,
                    variable: EnvironmentVariable {
                        id: String::new(),
                        key,
                        value,
                        enabled: params.enabled.unwrap_or(true),
                        secret: params.secret.unwrap_or(false),
                        created_by: None,
                    },
                },
                vec![ENVIRONMENTS_READ, ENVIRONMENTS_UPDATE],
            ))
        }
    }

    fn finish_remote_control_call(
        &mut self,
        target: ActiveUpstreamWorkspace,
        generation: u64,
        result: Result<Result<RemoteControlOutcome, String>, tokio::task::JoinError>,
        cx: &mut Context<Self>,
    ) -> ControlResponse {
        if self.workspace_switch_generation != generation {
            return ControlResponse::error(
                "The active workspace changed while the MCP operation was running; re-read state before retrying.",
            );
        }
        self.workspace_switch_abort_handle = None;
        let expected_provider = WorkspaceProviderId::Upstream {
            upstream_id: target.upstream_id.clone(),
            workspace_id: target.workspace_id.clone(),
        };
        if self.workspace_providers.active_id() != &expected_provider {
            self.workspace_switch_status = WorkspaceSwitchStatus::Idle;
            return ControlResponse::error(
                "The active workspace changed while the MCP operation was running; re-read state before retrying.",
            );
        }
        let outcome = match result {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(error)) => return self.fail_remote_control_call(error, cx),
            Err(error) if error.is_cancelled() => {
                return self.fail_remote_control_call(
                    "The remote MCP operation was cancelled; re-read state before retrying."
                        .to_owned(),
                    cx,
                );
            }
            Err(error) => {
                return self.fail_remote_control_call(
                    format!("The remote MCP operation could not be completed: {error}"),
                    cx,
                );
            }
        };

        let candidate = match outcome.snapshot {
            RemoteControlSnapshot::Workspace(remote) => {
                if remote.id != target.workspace_id {
                    return self.fail_remote_control_call(
                        "The server returned a different workspace.".to_owned(),
                        cx,
                    );
                }
                let mut workspace = remote.into_local_workspace();
                workspace.environments = self.workspace.environments.clone();
                workspace.active_environment_id = self.workspace.active_environment_id.clone();
                workspace
            }
            RemoteControlSnapshot::Environments(environments) => {
                if environments
                    .iter()
                    .any(|environment| environment.workspace_id != target.workspace_id)
                {
                    return self.fail_remote_control_call(
                        "The server returned an environment from another workspace.".to_owned(),
                        cx,
                    );
                }
                let mut workspace = self.workspace.clone();
                workspace.environments = environments
                    .into_iter()
                    .map(UpstreamEnvironmentView::into_local)
                    .collect();
                if workspace
                    .active_environment_id
                    .as_deref()
                    .is_some_and(|id| workspace.environment(id).is_none())
                {
                    workspace.active_environment_id = None;
                }
                workspace
            }
        };
        if let Err(error) = candidate.validate() {
            return self.fail_remote_control_call(
                format!("The server returned an invalid workspace: {error}"),
                cx,
            );
        }
        self.replace_active_remote_workspace(candidate);
        self.refresh_variable_intelligence(cx);
        self.workspace_switch_status = WorkspaceSwitchStatus::Idle;
        self.workspace_warning = None;
        cx.notify();

        let result = match outcome.result {
            RemoteControlResult::Collection(id) => json!({ "collection_id": id }),
            RemoteControlResult::Request(id) => self
                .workspace
                .saved_request(&id)
                .map(|(collection, request)| request_value(collection, request))
                .unwrap_or_else(|| json!({ "request_id": id })),
            RemoteControlResult::Environment(id) => self
                .workspace
                .environment(&id)
                .map(environment_value)
                .unwrap_or_else(|| json!({ "environment_id": id })),
            RemoteControlResult::Variable {
                environment_id,
                variable_id,
            } => self
                .workspace
                .environment(&environment_id)
                .and_then(|environment| {
                    environment
                        .variables
                        .iter()
                        .find(|variable| variable.id == variable_id)
                })
                .map(variable_value)
                .unwrap_or_else(|| json!({ "variable_id": variable_id })),
        };
        ControlResponse::success(result)
    }

    fn fail_remote_control_call(
        &mut self,
        error: String,
        cx: &mut Context<Self>,
    ) -> ControlResponse {
        self.workspace_switch_status = WorkspaceSwitchStatus::Error(error.clone());
        self.workspace_warning = Some(error.clone());
        cx.notify();
        ControlResponse::error(error)
    }

    pub(super) fn handle_control_call(
        &mut self,
        method: &str,
        params: Value,
        cx: &mut Context<Self>,
    ) -> ControlResponse {
        if method == "__list_enabled_tools" {
            return ControlResponse::success(json!({
                "tools": crate::control_tools::CONTROL_TOOLS
                    .iter()
                    .filter(|tool| self.control_tool_advertised(tool.name))
                    .map(|tool| tool.name)
                    .collect::<Vec<_>>()
            }));
        }
        if let Some(response) = self.validate_control_method(method) {
            return response;
        }
        let result = match method {
            "status" => self.control_status(),
            "list_workspaces" => self.control_list_workspaces(),
            "list_collections" => self.control_list_collections(),
            "create_collection" => self.control_create_collection(params, cx),
            "search_requests" => self.control_search_requests(params),
            "get_request" => self.control_get_request(params),
            "create_request" => self.control_create_request(params, cx),
            "save_request" => self.control_save_request(params, cx),
            "set_request_scripts" => self.control_set_request_scripts(params, cx),
            "get_http_exchange" => self.control_get_http_exchange(params),
            "cancel_http_request" => self.control_cancel_http_request(cx),
            "list_request_history" => self.control_list_request_history(params),
            "get_script_console" => self.control_get_script_console(params),
            "connect_websocket" => self.control_connect_websocket(params, cx),
            "send_websocket_message" => self.control_send_websocket_message(params, cx),
            "get_websocket_events" => self.control_get_websocket_events(params),
            "run_websocket_replay" => self.control_run_websocket_replay(params),
            "disconnect_websocket" => self.control_disconnect_websocket(params),
            "execute_http_request" | "run_script_console" => Err(format!(
                "the MCP tool '{method}' requires an active Resolved window"
            )),
            "list_environments" => self.control_list_environments(),
            "get_environment" => self.control_get_environment(params),
            "create_environment" => self.control_create_environment(params, cx),
            "rename_environment" => self.control_rename_environment(params, cx),
            "set_active_environment" => self.control_set_active_environment(params, cx),
            "set_environment_variable" => self.control_set_environment_variable(params, cx),
            _ => unreachable!("the control tool catalog and handler must stay aligned"),
        };
        match result {
            Ok(result) => ControlResponse::success(result),
            Err(error) => ControlResponse::error(error),
        }
    }

    fn control_status(&self) -> Result<Value, String> {
        let remote = matches!(
            self.workspace_providers.active_id(),
            WorkspaceProviderId::Upstream { .. }
        );
        Ok(json!({
            "product": PRODUCT_NAME,
            "version": env!("CARGO_PKG_VERSION"),
            "protocol_version": crate::control_server::CONTROL_PROTOCOL_VERSION,
            "active_workspace": self.workspace_providers.active_id().to_string(),
            "workspace_provider": if remote { "remote" } else { "local" },
            "workspace_writable": if remote {
                self.settings.mcp.allow_remote_workspaces && self.remote_control_can_write()
            } else {
                self.workspace_writable
            },
            "remote_workspace_access": self.settings.mcp.allow_remote_workspaces,
            "mcp_websocket": self.mcp_websocket.as_ref().map(|connection| json!({
                "connection_id": connection.id,
                "request_id": connection.request_id,
                "state": connection.status.as_str()
            })),
            "enabled_tools": crate::control_tools::CONTROL_TOOLS
                .iter()
                .filter(|tool| self.control_tool_advertised(tool.name))
                .map(|tool| tool.name)
                .collect::<Vec<_>>()
        }))
    }

    fn control_list_workspaces(&self) -> Result<Value, String> {
        let active = self.workspace_providers.active_id().to_string();
        let mut workspaces = self
            .local_workspaces
            .iter()
            .map(|workspace| {
                let id = format!("local:{}", workspace.id);
                json!({
                    "id": id,
                    "name": workspace.name,
                    "active": id == active,
                    "provider": "local",
                    "writable": true
                })
            })
            .collect::<Vec<_>>();
        for profile in &self.settings.upstreams.servers {
            let writable =
                self.settings.mcp.allow_remote_workspaces && remote_profile_can_write(profile);
            for workspace in &profile.workspaces {
                let id = format!("upstream:{}:{}", profile.id, workspace.id);
                workspaces.push(json!({
                    "id": id,
                    "name": workspace.name,
                    "active": id == active,
                    "provider": "remote",
                    "server_id": profile.id,
                    "server": profile.display_label(),
                    "writable": writable
                }));
            }
        }
        Ok(json!({ "workspaces": workspaces, "active_workspace": active }))
    }

    fn remote_control_can_write(&self) -> bool {
        let WorkspaceProviderId::Upstream { upstream_id, .. } =
            self.workspace_providers.active_id()
        else {
            return false;
        };
        self.settings
            .upstreams
            .server(upstream_id)
            .is_some_and(remote_profile_can_write)
    }

    fn control_create_collection(
        &mut self,
        params: Value,
        cx: &mut Context<Self>,
    ) -> Result<Value, String> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Params {
            name: String,
        }
        let params: Params = decode(params)?;
        let mut candidate = self.workspace.clone();
        let id = candidate
            .create_collection(params.name)
            .map_err(|error| error.to_string())?;
        self.commit_control_workspace(candidate, cx)?;
        Ok(json!({ "collection_id": id }))
    }

    fn control_list_collections(&self) -> Result<Value, String> {
        let collections = self
            .workspace
            .collections
            .iter()
            .map(|collection| {
                json!({
                    "id": collection.id,
                    "name": collection.name,
                    "folders": collection.folders,
                    "requests": collection
                        .requests
                        .iter()
                        .map(|request| request_summary(collection, request))
                        .collect::<Vec<_>>()
                })
            })
            .collect::<Vec<_>>();
        Ok(json!({ "collections": collections }))
    }

    fn control_search_requests(&self, params: Value) -> Result<Value, String> {
        #[derive(Deserialize, Default)]
        #[serde(default, deny_unknown_fields)]
        struct Params {
            query: String,
        }
        let query = decode::<Params>(params)?.query.to_lowercase();
        let requests = self
            .workspace
            .collections
            .iter()
            .flat_map(|collection| {
                collection.requests.iter().filter_map(|request| {
                    let matches = query.is_empty()
                        || request.name.to_lowercase().contains(&query)
                        || request
                            .definition
                            .request
                            .url
                            .to_lowercase()
                            .contains(&query);
                    matches.then(|| request_summary(collection, request))
                })
            })
            .collect::<Vec<_>>();
        Ok(json!({ "requests": requests }))
    }

    fn control_get_request(&self, params: Value) -> Result<Value, String> {
        let request_id = required_string(&params, "request_id")?;
        let (collection, request) = self
            .workspace
            .saved_request(&request_id)
            .ok_or_else(|| format!("request '{request_id}' was not found"))?;
        Ok(request_value(collection, request))
    }

    fn control_create_request(
        &mut self,
        params: Value,
        cx: &mut Context<Self>,
    ) -> Result<Value, String> {
        let params: CreateRequestParams = decode(params)?;
        let mut candidate = self.workspace.clone();
        let definition = request_definition(params.request, params.websocket, params.scripts)?;
        let id = candidate
            .create_saved_request_in_folder(
                &params.collection_id,
                params.folder_id.as_deref(),
                params.name,
                definition,
            )
            .map_err(|error| error.to_string())?;
        self.commit_control_workspace(candidate, cx)?;
        let (collection, request) = self
            .workspace
            .saved_request(&id)
            .expect("created request exists");
        Ok(request_value(collection, request))
    }

    fn control_save_request(
        &mut self,
        params: Value,
        cx: &mut Context<Self>,
    ) -> Result<Value, String> {
        let params: SaveRequestParams = decode(params)?;
        let (collection, existing) = self
            .workspace
            .saved_request(&params.request_id)
            .ok_or_else(|| format!("request '{}' was not found", params.request_id))?;
        ensure_request_revision(existing, &params.expected_updated_at)?;
        let collection_id = collection.id.clone();
        let mut definition = existing.definition.clone();
        if let Some(request) = params.request {
            definition.request = request;
            definition.websocket = None;
        }
        if let Some(websocket) = params.websocket {
            definition.request = RequestDraft::default();
            definition.websocket = Some(websocket);
        } else if params.clear_websocket {
            definition.websocket = None;
        }
        if let Some(scripts) = params.scripts {
            definition.scripts = scripts;
        }
        let mut candidate = self.workspace.clone();
        candidate
            .update_saved_request(&collection_id, &params.request_id, definition)
            .map_err(|error| error.to_string())?;
        if let Some(name) = params.name {
            candidate
                .rename_saved_request(&collection_id, &params.request_id, name)
                .map_err(|error| error.to_string())?;
        }
        self.commit_control_workspace(candidate, cx)?;
        let (collection, request) = self
            .workspace
            .saved_request(&params.request_id)
            .expect("saved request exists");
        Ok(request_value(collection, request))
    }

    fn control_set_request_scripts(
        &mut self,
        params: Value,
        cx: &mut Context<Self>,
    ) -> Result<Value, String> {
        let params: SetScriptsParams = decode(params)?;
        self.control_save_request(
            json!({
                "request_id": params.request_id,
                "expected_updated_at": params.expected_updated_at,
                "scripts": {
                    "pre_request": params.pre_request,
                    "post_response": params.post_response
                }
            }),
            cx,
        )
    }

    pub(super) fn control_get_http_exchange(&self, params: Value) -> Result<Value, String> {
        #[derive(Deserialize)]
        #[serde(default, deny_unknown_fields)]
        struct Params {
            operation_id: Option<u64>,
            max_body_bytes: usize,
        }
        impl Default for Params {
            fn default() -> Self {
                Self {
                    operation_id: None,
                    max_body_bytes: 256 * 1024,
                }
            }
        }
        let params: Params = decode(params)?;
        let operation_id = params
            .operation_id
            .or(self.mcp_http_operation_id)
            .unwrap_or(self.request_generation);
        if let Some(snapshot) = self
            .mcp_http_exchange
            .as_ref()
            .filter(|snapshot| snapshot.operation_id == operation_id)
        {
            return Ok(http_exchange_value(
                snapshot,
                params.max_body_bytes.min(512 * 1024),
            ));
        }
        if Some(operation_id) != self.mcp_http_operation_id
            || operation_id != self.request_generation
        {
            return Err(format!(
                "HTTP operation {operation_id} is no longer current"
            ));
        }
        let max_body_bytes = params.max_body_bytes.min(512 * 1024);
        Ok(http_exchange_value(
            &McpHttpExchangeSnapshot {
                operation_id,
                state: if self.sending { "running" } else { "idle" },
                stage: self.execution_stage.map(ExecutionStage::label),
                request: self.response_request.clone(),
                response: self.response.clone(),
                error: self.request_error.clone(),
                diagnostic: self.script_diagnostic.clone(),
                pre_request_report: self.pre_script_report.clone(),
                post_response_report: self.post_script_report.clone(),
            },
            max_body_bytes,
        ))
    }

    fn control_cancel_http_request(&mut self, cx: &mut Context<Self>) -> Result<Value, String> {
        if self.script_console_running {
            if let Some(cancellation) = self.script_cancellation.as_ref() {
                cancellation.cancel();
            }
            return Ok(json!({ "cancelled": true, "operation": "script_console" }));
        }
        if !self.sending {
            return Ok(json!({ "cancelled": false, "reason": "no HTTP request is running" }));
        }
        self.cancel_request(cx);
        Ok(json!({ "cancelled": true, "operation": "http" }))
    }

    fn control_list_request_history(&self, params: Value) -> Result<Value, String> {
        #[derive(Deserialize)]
        #[serde(default, deny_unknown_fields)]
        struct Params {
            limit: usize,
        }
        impl Default for Params {
            fn default() -> Self {
                Self { limit: 25 }
            }
        }
        let limit = decode::<Params>(params)?.limit.min(100);
        Ok(json!({
            "history": self.history.entries().iter().take(limit).collect::<Vec<_>>()
        }))
    }

    pub(super) fn control_get_script_console(&self, params: Value) -> Result<Value, String> {
        #[derive(Deserialize, Default)]
        #[serde(default, deny_unknown_fields)]
        struct Params {
            operation_id: Option<u64>,
        }
        let operation_id = decode::<Params>(params)?.operation_id;
        if let Some(operation_id) = operation_id
            && Some(operation_id) != self.mcp_script_console_operation_id
        {
            return Err(format!(
                "script-console operation {operation_id} is no longer current"
            ));
        }
        let model = crate::app::script_console::script_console_model(
            self.pre_script_report.as_ref(),
            self.post_script_report.as_ref(),
            self.script_diagnostic.as_ref(),
            self.request_error.as_deref(),
        );
        Ok(json!({
            "operation_id": self.mcp_script_console_operation_id,
            "state": if self.script_console_running { "running" } else { "idle" },
            "output": model.copy_all_text(),
            "diagnostic": self.script_diagnostic.as_ref().map(script_diagnostic_value),
            "pre_request_report": self.pre_script_report.as_ref().map(script_report_value),
            "post_response_report": self.post_script_report.as_ref().map(script_report_value)
        }))
    }

    fn control_connect_websocket(
        &mut self,
        params: Value,
        cx: &mut Context<Self>,
    ) -> Result<Value, String> {
        let request_id = required_string(&params, "request_id")?;
        let (_, saved) = self
            .workspace
            .saved_request(&request_id)
            .ok_or_else(|| format!("request '{request_id}' was not found"))?;
        let document = saved.definition.websocket.clone().ok_or_else(|| {
            format!("request '{request_id}' is an HTTP request; use execute_http_request")
        })?;
        let connection_draft = RequestDraft {
            url: document.url.trim().to_owned(),
            headers: document.headers.clone(),
            ..RequestDraft::default()
        };
        let resolved = resolve_request(&connection_draft, self.workspace.active_environment())
            .map_err(|error| error.to_string())?;
        if resolved.request.url.is_empty() {
            return Err("The WebSocket URL cannot be empty.".to_owned());
        }
        let upstream_target = match self.workspace_providers.active_id() {
            WorkspaceProviderId::Local(_) => None,
            WorkspaceProviderId::Upstream { .. } => {
                if !self.active_upstream_has_permission(WORKSPACES_READ) {
                    return Err(format!(
                        "The signed-in server user lacks the '{WORKSPACES_READ}' permission required by 'connect_websocket'."
                    ));
                }
                Some(self.active_upstream_workspace()?)
            }
        };

        if let Some(connection) = self.mcp_websocket.take() {
            let _ = connection.sender.send(WebSocketCommand::Close);
            connection.abort_handle.abort();
        }
        self.mcp_websocket_generation = self.mcp_websocket_generation.wrapping_add(1).max(1);
        let connection_id = self.mcp_websocket_generation;
        let (command_sender, command_receiver) = tokio::sync::mpsc::unbounded_channel();
        let (wire_sender, mut wire_receiver) = tokio::sync::mpsc::unbounded_channel();
        let (event_sender, mut event_receiver) = tokio::sync::mpsc::unbounded_channel();
        let url = resolved.request.url;
        let headers = resolved.request.headers;
        let vault = self.credential_vault.clone();
        let upstream_client = self.upstream_execution_client.clone();
        let connection_wire_sender = wire_sender.clone();
        let task = self.runtime.spawn(async move {
            let result = match upstream_target {
                None => {
                    run_websocket_connection(
                        &url,
                        &headers,
                        command_receiver,
                        connection_wire_sender.clone(),
                    )
                    .await
                }
                Some(target) => {
                    let upstream_id = target.upstream_id.clone();
                    let credential = match tokio::task::spawn_blocking(move || {
                        vault.load_upstream(&upstream_id)
                    })
                    .await
                    {
                        Ok(Ok(Some(credential))) if credential.expires_at > Utc::now() => {
                            credential
                        }
                        Ok(Ok(_)) => {
                            let _ = connection_wire_sender.send(WebSocketSignal::Failed(
                                "Log in to this server again.".to_owned(),
                            ));
                            return;
                        }
                        Ok(Err(error)) => {
                            let _ = connection_wire_sender
                                .send(WebSocketSignal::Failed(error.to_string()));
                            return;
                        }
                        Err(error) => {
                            let _ = connection_wire_sender
                                .send(WebSocketSignal::Failed(error.to_string()));
                            return;
                        }
                    };
                    match get_upstream_execution_policy(
                        &upstream_client,
                        &target.base_url,
                        credential.bearer_token(),
                    )
                    .await
                    {
                        Ok(RequestExecutionMode::Local) => {
                            run_websocket_connection(
                                &url,
                                &headers,
                                command_receiver,
                                connection_wire_sender.clone(),
                            )
                            .await
                        }
                        Ok(RequestExecutionMode::Server) => {
                            run_upstream_websocket_connection(
                                &target.base_url,
                                credential.bearer_token(),
                                &target.workspace_id,
                                &url,
                                &headers,
                                command_receiver,
                                connection_wire_sender.clone(),
                            )
                            .await
                        }
                        Err(error) => {
                            let _ = connection_wire_sender
                                .send(WebSocketSignal::Failed(error.to_string()));
                            return;
                        }
                    }
                }
            };
            if let Err(error) = result {
                let _ = connection_wire_sender.send(WebSocketSignal::Failed(error.to_string()));
            }
        });
        let abort_handle = task.abort_handle();

        let automation_enabled = document.automation_enabled;
        let automation_source = document.automation_source;
        let automation_environment = self.workspace.active_environment().cloned();
        let automation_values = automation_environment
            .as_ref()
            .map(|environment| {
                environment
                    .variables
                    .iter()
                    .filter(|variable| variable.enabled)
                    .map(|variable| (variable.key.clone(), variable.value.clone()))
                    .collect::<std::collections::BTreeMap<_, _>>()
            })
            .unwrap_or_default();
        let automation_commands = command_sender.clone();
        let automation_events = event_sender.clone();
        self.runtime.spawn(async move {
            while let Some(signal) = wire_receiver.recv().await {
                let automation_event = match &signal {
                    WebSocketSignal::Connected => Some(WebSocketAutomationEvent::opened()),
                    WebSocketSignal::Text(payload) => {
                        Some(WebSocketAutomationEvent::text(payload.clone()))
                    }
                    WebSocketSignal::Binary(bytes) => Some(WebSocketAutomationEvent {
                        event_type: "message".to_owned(),
                        data: None,
                        binary_base64: Some(
                            base64::engine::general_purpose::STANDARD.encode(bytes),
                        ),
                    }),
                    _ => None,
                };
                if automation_events
                    .send(ControlWebSocketIncoming::Wire(signal))
                    .is_err()
                {
                    return;
                }
                if !automation_enabled {
                    continue;
                }
                let Some(automation_event) = automation_event else {
                    continue;
                };
                let source = automation_source.clone();
                let values = automation_values.clone();
                let output = tokio::task::spawn_blocking(move || {
                    execute_websocket_automation(&source, &automation_event, &values)
                })
                .await;
                match output {
                    Ok(Ok(output)) => {
                        for log in output.logs {
                            let _ =
                                automation_events.send(ControlWebSocketIncoming::ScriptLog(log));
                        }
                        for payload in output.sends {
                            let resolved = resolve_request(
                                &RequestDraft {
                                    url: payload,
                                    ..RequestDraft::default()
                                },
                                automation_environment.as_ref(),
                            );
                            match resolved {
                                Ok(resolved) => {
                                    let payload = resolved.request.url;
                                    if automation_commands
                                        .send(WebSocketCommand::SendText(payload.clone()))
                                        .is_ok()
                                    {
                                        let _ = automation_events
                                            .send(ControlWebSocketIncoming::SentText(payload));
                                    }
                                }
                                Err(error) => {
                                    let _ = automation_events.send(
                                        ControlWebSocketIncoming::ScriptError(error.to_string()),
                                    );
                                }
                            }
                        }
                    }
                    Ok(Err(error)) => {
                        let _ =
                            automation_events.send(ControlWebSocketIncoming::ScriptError(error));
                    }
                    Err(error) => {
                        let _ = automation_events
                            .send(ControlWebSocketIncoming::ScriptError(error.to_string()));
                    }
                }
            }
        });

        self.mcp_websocket = Some(ControlWebSocketConnection {
            id: connection_id,
            request_id,
            status: ControlWebSocketStatus::Connecting,
            notice: None,
            sender: command_sender,
            event_sender,
            abort_handle,
            events: Vec::new(),
            next_event_id: 1,
        });
        cx.spawn(async move |weak_this, cx| {
            while let Some(event) = event_receiver.recv().await {
                let Some(this) = weak_this.upgrade() else {
                    break;
                };
                let _ = this.update(cx, |this, cx| {
                    this.handle_control_websocket_event(connection_id, event);
                    cx.notify();
                });
            }
        })
        .detach();
        cx.notify();
        Ok(json!({
            "connection_id": connection_id,
            "request_id": self.mcp_websocket.as_ref().map(|connection| &connection.request_id),
            "state": "connecting"
        }))
    }

    fn control_send_websocket_message(
        &mut self,
        params: Value,
        cx: &mut Context<Self>,
    ) -> Result<Value, String> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Params {
            connection_id: u64,
            #[serde(default)]
            text: Option<String>,
            #[serde(default)]
            binary_base64: Option<String>,
            #[serde(default)]
            saved_message_id: Option<String>,
            #[serde(default)]
            template_id: Option<String>,
            #[serde(default)]
            template_values: std::collections::BTreeMap<String, String>,
        }
        let params: Params = decode(params)?;
        let connection = self
            .mcp_websocket
            .as_ref()
            .ok_or_else(|| "No MCP WebSocket connection exists.".to_owned())?;
        if connection.id != params.connection_id {
            return Err(format!(
                "WebSocket connection {} is no longer current",
                params.connection_id
            ));
        }
        if connection.status != ControlWebSocketStatus::Connected {
            return Err(format!(
                "WebSocket connection {} is {}",
                connection.id,
                connection.status.as_str()
            ));
        }
        let request_id = connection.request_id.clone();
        let sender = connection.sender.clone();
        let selected_sources = [
            params.text.is_some(),
            params.binary_base64.is_some(),
            params.saved_message_id.is_some(),
            params.template_id.is_some(),
        ]
        .into_iter()
        .filter(|selected| *selected)
        .count();
        if selected_sources != 1 {
            return Err(
                "provide exactly one of 'text', 'binary_base64', 'saved_message_id', or 'template_id'"
                    .to_owned(),
            );
        }
        let resolve_text = |text: String| -> Result<String, String> {
            resolve_request(
                &RequestDraft {
                    url: text,
                    ..RequestDraft::default()
                },
                self.workspace.active_environment(),
            )
            .map(|resolved| resolved.request.url)
            .map_err(|error| error.to_string())
        };
        let mut commands = if let Some(text) = params.text {
            vec![WebSocketCommand::SendText(resolve_text(text)?)]
        } else if let Some(encoded) = params.binary_base64 {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .map_err(|error| format!("invalid base64 WebSocket payload: {error}"))?;
            if bytes.len() > crate::core::MAX_WEBSOCKET_MESSAGE_BYTES {
                return Err(format!(
                    "WebSocket payload exceeds the {}-byte limit",
                    crate::core::MAX_WEBSOCKET_MESSAGE_BYTES
                ));
            }
            vec![WebSocketCommand::SendBinary(bytes)]
        } else {
            let (_, saved) = self
                .workspace
                .saved_request(&request_id)
                .ok_or_else(|| format!("request '{request_id}' was not found"))?;
            let document =
                saved.definition.websocket.as_ref().ok_or_else(|| {
                    "The saved request is no longer a WebSocket document.".to_owned()
                })?;
            if let Some(message_id) = params.saved_message_id {
                let message = document
                    .messages
                    .iter()
                    .find(|message| message.id == message_id)
                    .ok_or_else(|| format!("saved message '{message_id}' was not found"))?;
                let payload = resolve_text(message.payload.clone())?;
                if message.language == RawBodyLanguage::JsonLines {
                    crate::core::parse_json_lines(&payload)?
                        .into_iter()
                        .map(|record| WebSocketCommand::SendText(payload[record.range].to_owned()))
                        .collect()
                } else {
                    vec![WebSocketCommand::SendText(payload)]
                }
            } else {
                let template_id = params.template_id.expect("one source was selected");
                let template = document
                    .templates
                    .iter()
                    .find(|template| template.id == template_id)
                    .ok_or_else(|| format!("template '{template_id}' was not found"))?;
                let rendered = render_message_template(&template.payload, &params.template_values)
                    .map_err(|error| error.to_string())?;
                vec![WebSocketCommand::SendText(resolve_text(rendered)?)]
            }
        };
        let sent_count = commands.len();
        let connection = self.control_websocket_mut(params.connection_id)?;
        for command in commands.drain(..) {
            sender
                .send(command.clone())
                .map_err(|_| "The WebSocket connection is no longer available.".to_owned())?;
            match command {
                WebSocketCommand::SendText(payload) => {
                    connection.push_event("sent", "text", Some(payload))
                }
                WebSocketCommand::SendBinary(bytes) => {
                    connection.push_binary_event("sent", "binary", bytes)
                }
                WebSocketCommand::Close => unreachable!(),
            }
        }
        cx.notify();
        Ok(json!({
            "sent": true,
            "sent_count": sent_count,
            "connection_id": params.connection_id
        }))
    }

    pub(super) fn control_get_websocket_events(&self, params: Value) -> Result<Value, String> {
        #[derive(Deserialize)]
        #[serde(default, deny_unknown_fields)]
        struct Params {
            connection_id: Option<u64>,
            after_event_id: u64,
            limit: usize,
            max_payload_bytes: usize,
        }
        impl Default for Params {
            fn default() -> Self {
                Self {
                    connection_id: None,
                    after_event_id: 0,
                    limit: 100,
                    max_payload_bytes: 64 * 1024,
                }
            }
        }
        let params: Params = decode(params)?;
        let connection = self
            .mcp_websocket
            .as_ref()
            .ok_or_else(|| "No MCP WebSocket connection exists.".to_owned())?;
        if params.connection_id.is_some_and(|id| id != connection.id) {
            return Err(format!(
                "WebSocket connection {} is no longer current",
                params.connection_id.unwrap()
            ));
        }
        let mut remaining_bytes = 512 * 1024;
        let mut events = Vec::new();
        for event in connection
            .events
            .iter()
            .filter(|event| event.id > params.after_event_id)
            .take(params.limit.min(500))
        {
            let event_limit = params
                .max_payload_bytes
                .min(256 * 1024)
                .min(remaining_bytes);
            events.push(control_websocket_event_value(event, event_limit));
            let event_size = event
                .payload
                .as_ref()
                .map(String::len)
                .or_else(|| event.binary.as_ref().map(Vec::len))
                .unwrap_or(0);
            remaining_bytes = remaining_bytes.saturating_sub(event_size.min(event_limit));
            if remaining_bytes == 0 {
                break;
            }
        }
        Ok(json!({
            "connection_id": connection.id,
            "request_id": connection.request_id,
            "state": connection.status.as_str(),
            "notice": connection.notice,
            "events": events,
            "last_event_id": connection.events.last().map(|event| event.id).unwrap_or(0)
        }))
    }

    fn control_run_websocket_replay(&mut self, params: Value) -> Result<Value, String> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Params {
            connection_id: u64,
            replay_id: String,
        }
        let params: Params = decode(params)?;
        let connection = self.control_websocket_mut(params.connection_id)?;
        if connection.status != ControlWebSocketStatus::Connected {
            return Err(format!(
                "WebSocket connection {} is {}",
                connection.id,
                connection.status.as_str()
            ));
        }
        let request_id = connection.request_id.clone();
        let sender = connection.sender.clone();
        let events = connection.event_sender.clone();
        let (_, saved) = self
            .workspace
            .saved_request(&request_id)
            .ok_or_else(|| format!("request '{request_id}' was not found"))?;
        let document = saved
            .definition
            .websocket
            .as_ref()
            .ok_or_else(|| "The saved request is no longer a WebSocket document.".to_owned())?;
        let replay = document
            .replays
            .iter()
            .find(|replay| replay.id == params.replay_id)
            .ok_or_else(|| format!("replay '{}' was not found", params.replay_id))?;
        let environment = self.workspace.active_environment();
        let mut frames = Vec::with_capacity(replay.frames.len());
        for frame in &replay.frames {
            let resolved = resolve_request(
                &RequestDraft {
                    url: frame.payload.clone(),
                    ..RequestDraft::default()
                },
                environment,
            )
            .map_err(|error| error.to_string())?;
            frames.push((frame.delay_ms, resolved.request.url));
        }
        self.runtime.spawn(async move {
            for (delay_ms, payload) in frames {
                if delay_ms > 0 {
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                }
                if sender
                    .send(WebSocketCommand::SendText(payload.clone()))
                    .is_err()
                {
                    break;
                }
                let _ = events.send(ControlWebSocketIncoming::SentText(payload));
            }
        });
        Ok(json!({
            "started": true,
            "connection_id": params.connection_id,
            "replay_id": params.replay_id,
            "frame_count": replay.frames.len()
        }))
    }

    fn control_disconnect_websocket(&mut self, params: Value) -> Result<Value, String> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Params {
            connection_id: u64,
        }
        let params: Params = decode(params)?;
        let connection = self.control_websocket_mut(params.connection_id)?;
        if matches!(
            connection.status,
            ControlWebSocketStatus::Disconnected | ControlWebSocketStatus::Failed
        ) {
            return Ok(json!({ "closed": false, "state": connection.status.as_str() }));
        }
        if connection.status == ControlWebSocketStatus::Connecting {
            connection.abort_handle.abort();
            connection.status = ControlWebSocketStatus::Disconnected;
            connection.push_event("system", "close", Some("Connection cancelled".to_owned()));
        } else {
            connection
                .sender
                .send(WebSocketCommand::Close)
                .map_err(|_| "The WebSocket connection is no longer available.".to_owned())?;
            connection.status = ControlWebSocketStatus::Closing;
        }
        Ok(json!({ "closed": true, "state": connection.status.as_str() }))
    }

    fn control_websocket_mut(
        &mut self,
        connection_id: u64,
    ) -> Result<&mut ControlWebSocketConnection, String> {
        let connection = self
            .mcp_websocket
            .as_mut()
            .ok_or_else(|| "No MCP WebSocket connection exists.".to_owned())?;
        if connection.id != connection_id {
            return Err(format!(
                "WebSocket connection {connection_id} is no longer current"
            ));
        }
        Ok(connection)
    }

    fn handle_control_websocket_event(
        &mut self,
        connection_id: u64,
        incoming: ControlWebSocketIncoming,
    ) {
        let Some(connection) = self.mcp_websocket.as_mut() else {
            return;
        };
        if connection.id != connection_id {
            return;
        }
        match incoming {
            ControlWebSocketIncoming::Wire(WebSocketSignal::Connected) => {
                connection.status = ControlWebSocketStatus::Connected;
                connection.notice = None;
                connection.push_event("system", "open", Some("Connected".to_owned()));
            }
            ControlWebSocketIncoming::Wire(WebSocketSignal::Text(payload)) => {
                connection.push_event("received", "text", Some(payload));
            }
            ControlWebSocketIncoming::Wire(WebSocketSignal::Binary(bytes)) => {
                connection.push_binary_event("received", "binary", bytes);
            }
            ControlWebSocketIncoming::Wire(WebSocketSignal::Ping(bytes)) => {
                connection.push_binary_event("received", "ping", bytes);
            }
            ControlWebSocketIncoming::Wire(WebSocketSignal::Pong(bytes)) => {
                connection.push_binary_event("received", "pong", bytes);
            }
            ControlWebSocketIncoming::Wire(WebSocketSignal::Closed(reason)) => {
                connection.status = ControlWebSocketStatus::Disconnected;
                connection.push_event(
                    "system",
                    "close",
                    Some(reason.unwrap_or_else(|| "Connection closed".to_owned())),
                );
            }
            ControlWebSocketIncoming::Wire(WebSocketSignal::Failed(error)) => {
                connection.status = ControlWebSocketStatus::Failed;
                connection.notice = Some(error.clone());
                connection.push_event("system", "error", Some(error));
            }
            ControlWebSocketIncoming::SentText(payload) => {
                connection.push_event("sent", "text", Some(payload));
            }
            ControlWebSocketIncoming::ScriptLog(log) => {
                connection.push_event("system", "script", Some(log));
            }
            ControlWebSocketIncoming::ScriptError(error) => {
                connection.push_event("system", "script_error", Some(error));
            }
        }
    }

    fn control_list_environments(&self) -> Result<Value, String> {
        let environments = self
            .workspace
            .environments
            .iter()
            .map(environment_value)
            .collect::<Vec<_>>();
        Ok(json!({
            "environments": environments,
            "active_environment_id": self.workspace.active_environment_id
        }))
    }

    fn control_get_environment(&self, params: Value) -> Result<Value, String> {
        let id = required_string(&params, "environment_id")?;
        let environment = self
            .workspace
            .environment(&id)
            .ok_or_else(|| format!("environment '{id}' was not found"))?;
        Ok(environment_value(environment))
    }

    fn control_create_environment(
        &mut self,
        params: Value,
        cx: &mut Context<Self>,
    ) -> Result<Value, String> {
        let name = required_string(&params, "name")?;
        let mut candidate = self.workspace.clone();
        let id = candidate
            .create_environment(name)
            .map_err(|error| error.to_string())?;
        self.commit_control_workspace(candidate, cx)?;
        let environment = self
            .workspace
            .environment(&id)
            .expect("created environment exists");
        Ok(environment_value(environment))
    }

    fn control_rename_environment(
        &mut self,
        params: Value,
        cx: &mut Context<Self>,
    ) -> Result<Value, String> {
        let id = required_string(&params, "environment_id")?;
        let name = required_string(&params, "name")?;
        let mut candidate = self.workspace.clone();
        candidate
            .rename_environment(&id, name)
            .map_err(|error| error.to_string())?;
        self.commit_control_workspace(candidate, cx)?;
        Ok(environment_value(
            self.workspace
                .environment(&id)
                .expect("renamed environment exists"),
        ))
    }

    fn control_set_active_environment(
        &mut self,
        params: Value,
        cx: &mut Context<Self>,
    ) -> Result<Value, String> {
        let environment_id = params
            .get("environment_id")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let mut candidate = self.workspace.clone();
        candidate
            .set_active_environment(environment_id.as_deref())
            .map_err(|error| error.to_string())?;
        if matches!(
            self.workspace_providers.active_id(),
            WorkspaceProviderId::Upstream { .. }
        ) {
            if self.workspace_switch_status.busy() {
                return Err("The active workspace is busy; retry the MCP call shortly.".to_owned());
            }
            if !self.settings_writable {
                return Err(
                    "Settings are read-only; the remote environment selection could not be saved."
                        .to_owned(),
                );
            }
            self.persist_upstream_active_environment(environment_id.as_deref(), cx)?;
            self.replace_active_remote_workspace(candidate);
            self.refresh_variable_intelligence(cx);
            cx.notify();
        } else {
            self.commit_control_workspace(candidate, cx)?;
        }
        Ok(json!({ "active_environment_id": self.workspace.active_environment_id }))
    }

    fn control_set_environment_variable(
        &mut self,
        params: Value,
        cx: &mut Context<Self>,
    ) -> Result<Value, String> {
        let params: SetVariableParams = decode(params)?;
        let mut candidate = self.workspace.clone();
        let variable_id = if let Some(variable_id) = params.variable_id {
            let existing = candidate
                .environment(&params.environment_id)
                .and_then(|environment| {
                    environment
                        .variables
                        .iter()
                        .find(|variable| variable.id == variable_id)
                })
                .cloned()
                .ok_or_else(|| format!("variable '{variable_id}' was not found"))?;
            if existing.secret && params.secret == Some(false) {
                return Err(
                    "secret variables cannot be made non-secret through MCP; use the Resolved UI"
                        .to_owned(),
                );
            }
            candidate
                .update_environment_variable(
                    &params.environment_id,
                    &variable_id,
                    params.key.unwrap_or(existing.key),
                    params.value.unwrap_or(existing.value),
                    params.enabled.unwrap_or(existing.enabled),
                    params.secret.unwrap_or(existing.secret),
                )
                .map_err(|error| error.to_string())?;
            variable_id
        } else {
            let key = params
                .key
                .ok_or_else(|| "key is required when creating a variable".to_owned())?;
            let value = params
                .value
                .ok_or_else(|| "value is required when creating a variable".to_owned())?;
            candidate
                .add_environment_variable(
                    &params.environment_id,
                    key,
                    value,
                    params.enabled.unwrap_or(true),
                    params.secret.unwrap_or(false),
                )
                .map_err(|error| error.to_string())?
        };
        self.commit_control_workspace(candidate, cx)?;
        let environment = self
            .workspace
            .environment(&params.environment_id)
            .expect("environment exists");
        let variable = environment
            .variables
            .iter()
            .find(|variable| variable.id == variable_id)
            .expect("variable exists");
        Ok(variable_value(variable))
    }

    pub(super) fn commit_control_workspace(
        &mut self,
        candidate: Workspace,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        self.commit_workspace(candidate)?;
        self.refresh_variable_intelligence(cx);
        cx.notify();
        Ok(())
    }
}

fn is_mutating_control_method(method: &str) -> bool {
    crate::control_tools::tool(method).is_some_and(|tool| !tool.read_only)
}

impl ControlWebSocketConnection {
    fn push_event(&mut self, direction: &'static str, kind: &'static str, payload: Option<String>) {
        let id = self.next_event_id;
        self.next_event_id = id.wrapping_add(1).max(1);
        self.events.push(ControlWebSocketEvent {
            id,
            at: Utc::now(),
            direction,
            kind,
            payload,
            binary: None,
        });
        let excess = self
            .events
            .len()
            .saturating_sub(MAX_WEBSOCKET_TIMELINE_ENTRIES);
        if excess > 0 {
            self.events.drain(..excess);
        }
    }

    fn push_binary_event(&mut self, direction: &'static str, kind: &'static str, binary: Vec<u8>) {
        let id = self.next_event_id;
        self.next_event_id = id.wrapping_add(1).max(1);
        self.events.push(ControlWebSocketEvent {
            id,
            at: Utc::now(),
            direction,
            kind,
            payload: None,
            binary: Some(binary),
        });
        let excess = self
            .events
            .len()
            .saturating_sub(MAX_WEBSOCKET_TIMELINE_ENTRIES);
        if excess > 0 {
            self.events.drain(..excess);
        }
    }
}

fn remote_profile_can_write(profile: &UpstreamProfile) -> bool {
    (profile.has_permission(WORKSPACES_READ)
        && (profile.has_permission(COLLECTIONS_CREATE)
            || profile.has_permission(REQUESTS_CREATE)
            || profile.has_permission(REQUESTS_UPDATE)))
        || (profile.has_permission(ENVIRONMENTS_READ)
            && (profile.has_permission(ENVIRONMENTS_CREATE)
                || profile.has_permission(ENVIRONMENTS_UPDATE)
                || profile.has_permission(ENVIRONMENT_VALUES_UPDATE)))
}

fn decode<T: for<'de> Deserialize<'de>>(params: Value) -> Result<T, String> {
    serde_json::from_value(params).map_err(|error| format!("invalid parameters: {error}"))
}

fn required_string(params: &Value, name: &str) -> Result<String, String> {
    params
        .get(name)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| format!("'{name}' must be a non-empty string"))
}

fn ensure_request_revision(request: &SavedRequest, expected: &str) -> Result<(), String> {
    let actual = request.updated_at.to_rfc3339();
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "request '{}' changed since it was read (expected {expected}, current {actual})",
            request.id
        ))
    }
}

fn request_summary(collection: &Collection, request: &SavedRequest) -> Value {
    let websocket = request.definition.websocket.as_ref();
    json!({
        "id": request.id,
        "name": request.name,
        "collection_id": collection.id,
        "collection_name": collection.name,
        "folder_id": request.folder_id,
        "kind": if websocket.is_some() { "websocket" } else { "http" },
        "method": if websocket.is_some() { "WEBSOCKET" } else { request.definition.request.method.as_str() },
        "url": websocket.map(|document| document.url.as_str()).unwrap_or(request.definition.request.url.as_str()),
        "updated_at": request.updated_at.to_rfc3339()
    })
}

fn request_value(collection: &Collection, request: &SavedRequest) -> Value {
    let mut value = request_summary(collection, request);
    let object = value.as_object_mut().expect("request summary is an object");
    object.insert("request".to_owned(), json!(request.definition.request));
    object.insert("scripts".to_owned(), json!(request.definition.scripts));
    object.insert("websocket".to_owned(), json!(request.definition.websocket));
    object.insert(
        "created_at".to_owned(),
        json!(request.created_at.to_rfc3339()),
    );
    value
}

fn request_definition(
    request: Option<RequestDraft>,
    websocket: Option<WebSocketWorkspace>,
    scripts: RequestScripts,
) -> Result<RequestTemplate, String> {
    match (request, websocket) {
        (Some(request), None) => Ok(RequestTemplate::new(request).with_scripts(scripts)),
        (None, Some(websocket)) => Ok(RequestTemplate::websocket(websocket).with_scripts(scripts)),
        (Some(_), Some(_)) => Err("provide either 'request' or 'websocket', not both".to_owned()),
        (None, None) => Err("either 'request' or 'websocket' is required".to_owned()),
    }
}

fn environment_value(environment: &Environment) -> Value {
    json!({
        "id": environment.id,
        "name": environment.name,
        "variables": environment.variables.iter().map(variable_value).collect::<Vec<_>>()
    })
}

fn variable_value(variable: &EnvironmentVariable) -> Value {
    json!({
        "id": variable.id,
        "key": variable.key,
        "value": if variable.secret { Value::Null } else { json!(variable.value) },
        "has_value": !variable.value.is_empty(),
        "enabled": variable.enabled,
        "secret": variable.secret
    })
}

fn http_exchange_value(snapshot: &McpHttpExchangeSnapshot, max_body_bytes: usize) -> Value {
    json!({
        "operation_id": snapshot.operation_id,
        "state": snapshot.state,
        "stage": snapshot.stage,
        "request": snapshot.request,
        "response": snapshot.response.as_ref().map(|response| response_value(response, max_body_bytes)),
        "error": snapshot.error,
        "diagnostic": snapshot.diagnostic.as_ref().map(script_diagnostic_value),
        "pre_request_report": snapshot.pre_request_report.as_ref().map(script_report_value),
        "post_response_report": snapshot.post_response_report.as_ref().map(script_report_value)
    })
}

fn response_value(response: &ResponseData, max_body_bytes: usize) -> Value {
    let requested_bytes = response.body.len().min(max_body_bytes);
    let (included_bytes, body_encoding, body_value) = match std::str::from_utf8(&response.body) {
        Ok(text) => {
            let mut boundary = requested_bytes;
            while !text.is_char_boundary(boundary) {
                boundary -= 1;
            }
            (boundary, "utf8", text[..boundary].to_owned())
        }
        Err(_) => (
            requested_bytes,
            "base64",
            base64::engine::general_purpose::STANDARD.encode(&response.body[..requested_bytes]),
        ),
    };
    json!({
        "status": response.status,
        "status_text": response.status_text,
        "http_version": response.http_version,
        "final_url": response.final_url,
        "headers": response.headers.iter().map(|header| json!({
            "name": header.name,
            "value": header.value
        })).collect::<Vec<_>>(),
        "content_type": response.content_type,
        "duration_ms": response.duration.as_millis().min(u128::from(u64::MAX)) as u64,
        "size_bytes": response.size_bytes(),
        "body": body_value,
        "body_encoding": body_encoding,
        "body_included_bytes": included_bytes,
        "body_truncated": included_bytes < response.body.len()
    })
}

fn control_websocket_event_value(event: &ControlWebSocketEvent, max_payload_bytes: usize) -> Value {
    let (payload, binary_base64, size_bytes, included_bytes) = if let Some(payload) = &event.payload
    {
        let mut included = payload.len().min(max_payload_bytes);
        while !payload.is_char_boundary(included) {
            included -= 1;
        }
        (
            Some(payload[..included].to_owned()),
            None,
            payload.len(),
            included,
        )
    } else if let Some(binary) = &event.binary {
        let included = binary.len().min(max_payload_bytes);
        (
            None,
            Some(base64::engine::general_purpose::STANDARD.encode(&binary[..included])),
            binary.len(),
            included,
        )
    } else {
        (None, None, 0, 0)
    };
    json!({
        "id": event.id,
        "at": event.at.to_rfc3339(),
        "direction": event.direction,
        "kind": event.kind,
        "payload": payload,
        "binary_base64": binary_base64,
        "size_bytes": size_bytes,
        "included_bytes": included_bytes,
        "truncated": included_bytes < size_bytes
    })
}

fn script_report_value(report: &ScriptReport) -> Value {
    json!({
        "phase": report.phase.to_string(),
        "duration_ms": report.duration.as_millis().min(u128::from(u64::MAX)) as u64,
        "logs": report.logs,
        "tests": report.tests,
        "response_body_truncated": report.response_body_truncated
    })
}

fn script_diagnostic_value(diagnostic: &ScriptDiagnostic) -> Value {
    json!({
        "phase": diagnostic.phase.to_string(),
        "kind": format!("{:?}", diagnostic.kind).to_lowercase(),
        "filename": diagnostic.filename,
        "message": diagnostic.message,
        "stack": diagnostic.stack
    })
}

#[cfg(test)]
mod remote_tests {
    use super::*;
    use std::{
        io::{Read as _, Write as _},
        net::{TcpListener, TcpStream},
        thread,
    };

    #[test]
    fn remote_collection_mutation_uses_authenticated_api_and_refreshes_workspace() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            for expected in [
                "POST /api/v1/workspaces/workspace-1/collections HTTP/1.1\r\n",
                "GET /api/v1/workspaces/workspace-1 HTTP/1.1\r\n",
            ] {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                    .unwrap();
                let request = read_http_request(&mut stream);
                let header_end = request
                    .windows(4)
                    .position(|window| window == b"\r\n\r\n")
                    .unwrap();
                let headers = String::from_utf8(request[..header_end].to_vec()).unwrap();
                assert!(headers.starts_with(expected));
                assert!(
                    headers
                        .to_ascii_lowercase()
                        .contains("authorization: bearer remote-token\r\n")
                );

                let now = Utc::now();
                let data = if expected.starts_with("POST") {
                    json!({
                        "id": "collection-1",
                        "workspace_id": "workspace-1",
                        "parent_collection_id": null,
                        "name": "Agent collection",
                        "user_ids": [],
                        "sub_collections": [],
                        "requests": [],
                        "created_at": now,
                        "updated_at": now
                    })
                } else {
                    json!({
                        "id": "workspace-1",
                        "name": "Remote workspace",
                        "user_ids": [],
                        "collections": [{
                            "id": "collection-1",
                            "workspace_id": "workspace-1",
                            "parent_collection_id": null,
                            "name": "Agent collection",
                            "user_ids": [],
                            "sub_collections": [],
                            "requests": [],
                            "created_at": now,
                            "updated_at": now
                        }],
                        "created_at": now,
                        "updated_at": now
                    })
                };
                write_json_response(
                    &mut stream,
                    if expected.starts_with("POST") {
                        201
                    } else {
                        200
                    },
                    data,
                );
            }
        });

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let client = build_upstream_client().unwrap();
        let target = ActiveUpstreamWorkspace {
            upstream_id: "upstream-1".to_owned(),
            workspace_id: "workspace-1".to_owned(),
            base_url: url::Url::parse(&format!("http://{address}/")).unwrap(),
        };
        let outcome = runtime
            .block_on(
                RemoteControlMutation::CreateCollection {
                    name: "Agent collection".to_owned(),
                }
                .execute(&client, &target, "remote-token"),
            )
            .unwrap();
        server.join().unwrap();

        assert!(matches!(
            outcome.result,
            RemoteControlResult::Collection(ref id) if id == "collection-1"
        ));
        let RemoteControlSnapshot::Workspace(workspace) = outcome.snapshot else {
            panic!("collection mutation should refresh the workspace")
        };
        assert_eq!(workspace.collections.len(), 1);
        assert_eq!(workspace.collections[0].name, "Agent collection");
    }

    #[test]
    fn remote_request_update_rechecks_server_revision_before_writing() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let definition = RequestTemplate::default();
        let response_definition = definition.clone();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                .unwrap();
            let request = read_http_request(&mut stream);
            let header_end = request
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .unwrap();
            let headers = String::from_utf8(request[..header_end].to_vec()).unwrap();
            assert!(headers.starts_with("GET /api/v1/workspaces/workspace-1 HTTP/1.1\r\n"));
            let now = Utc::now();
            write_json_response(
                &mut stream,
                200,
                json!({
                    "id": "workspace-1",
                    "name": "Remote workspace",
                    "user_ids": [],
                    "collections": [{
                        "id": "collection-1",
                        "workspace_id": "workspace-1",
                        "parent_collection_id": null,
                        "name": "Requests",
                        "user_ids": [],
                        "sub_collections": [],
                        "requests": [{
                            "id": "request-1",
                            "collection_id": "collection-1",
                            "name": "Current request",
                            "definition": response_definition,
                            "created_at": now,
                            "updated_at": now
                        }],
                        "created_at": now,
                        "updated_at": now
                    }],
                    "created_at": now,
                    "updated_at": now
                }),
            );
        });

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let client = build_upstream_client().unwrap();
        let target = ActiveUpstreamWorkspace {
            upstream_id: "upstream-1".to_owned(),
            workspace_id: "workspace-1".to_owned(),
            base_url: url::Url::parse(&format!("http://{address}/")).unwrap(),
        };
        let error = runtime
            .block_on(
                RemoteControlMutation::SaveRequest {
                    target_collection_id: "collection-1".to_owned(),
                    request_id: "request-1".to_owned(),
                    expected_updated_at: "2000-01-01T00:00:00+00:00".to_owned(),
                    name: "Stale overwrite".to_owned(),
                    definition,
                }
                .execute(&client, &target, "remote-token"),
            )
            .err()
            .expect("stale remote revision must be rejected");
        server.join().unwrap();
        assert!(error.contains("changed since it was read"), "{error}");
    }

    fn read_http_request(stream: &mut TcpStream) -> Vec<u8> {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4096];
        let mut expected_length = None;
        loop {
            let read = stream.read(&mut buffer).unwrap();
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
            if let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&request[..header_end]).to_ascii_lowercase();
                let content_length = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .and_then(|value| value.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                expected_length = Some(header_end + 4 + content_length);
            }
            if expected_length.is_some_and(|length| request.len() >= length) {
                break;
            }
        }
        request
    }

    fn write_json_response(stream: &mut TcpStream, status: u16, data: Value) {
        let body = serde_json::to_vec(&json!({
            "request_id": "remote-control-test",
            "success": true,
            "data": data
        }))
        .unwrap();
        let status_text = if status == 201 {
            "201 Created"
        } else {
            "200 OK"
        };
        write!(
            stream,
            "HTTP/1.1 {status_text}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .unwrap();
        stream.write_all(&body).unwrap();
    }
}

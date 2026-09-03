use super::*;
use crate::control_server::{ControlCall, ControlResponse};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedReceiver;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateRequestParams {
    collection_id: String,
    #[serde(default)]
    folder_id: Option<String>,
    name: String,
    request: RequestDraft,
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
    pub(super) fn sync_control_server(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        let data_directory = self
            .database_store
            .path()
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."));
        if !self.settings.mcp.enabled {
            self._control_server = None;
            crate::control_server::ControlServer::remove_stale_files(data_directory)
                .map_err(|error| error.to_string())?;
            return Ok(());
        }
        if self._control_server.is_some() {
            return Ok(());
        }

        let (server, receiver) = crate::control_server::ControlServer::start(data_directory)
            .map_err(|error| error.to_string())?;
        self._control_server = Some(server);
        self.attach_control_plane(receiver, cx);
        Ok(())
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
                let dispatch = this
                    .update(cx, |this, cx| {
                        this.dispatch_control_call(&call.method, call.params.clone(), cx)
                    })
                    .unwrap_or_else(|error| {
                        ControlDispatch::Immediate(ControlResponse::error(error.to_string()))
                    });
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
                Ok((
                    RemoteControlMutation::CreateRequest {
                        target_collection_id,
                        name: params.name,
                        definition: RequestTemplate::new(params.request)
                            .with_scripts(params.scripts),
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
        let definition = RequestTemplate::new(params.request).with_scripts(params.scripts);
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

    fn commit_control_workspace(
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
    json!({
        "id": request.id,
        "name": request.name,
        "collection_id": collection.id,
        "collection_name": collection.name,
        "folder_id": request.folder_id,
        "method": request.definition.request.method,
        "url": request.definition.request.url,
        "updated_at": request.updated_at.to_rfc3339()
    })
}

fn request_value(collection: &Collection, request: &SavedRequest) -> Value {
    let mut value = request_summary(collection, request);
    let object = value.as_object_mut().expect("request summary is an object");
    object.insert("request".to_owned(), json!(request.definition.request));
    object.insert("scripts".to_owned(), json!(request.definition.scripts));
    object.insert(
        "created_at".to_owned(),
        json!(request.created_at.to_rfc3339()),
    );
    value
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

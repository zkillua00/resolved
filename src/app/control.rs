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
        cx.spawn(async move |this, cx| {
            while let Some(call) = receiver.recv().await {
                let Some(this) = this.upgrade() else {
                    call.respond(ControlResponse::error("Resolved is shutting down"));
                    break;
                };
                let response = this
                    .update(cx, |this, cx| {
                        this.handle_control_call(&call.method, call.params.clone(), cx)
                    })
                    .unwrap_or_else(|error| ControlResponse::error(error.to_string()));
                call.respond(response);
            }
        })
        .detach();
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
                    .filter(|tool| self.settings.mcp.tool_enabled(tool.name))
                    .map(|tool| tool.name)
                    .collect::<Vec<_>>()
            }));
        }
        if !self.settings.mcp.enabled {
            return ControlResponse::error("MCP is disabled in Resolved settings");
        }
        if crate::control_tools::tool(method).is_none() {
            return ControlResponse::error(format!("unknown local control method '{method}'"));
        }
        if !self.settings.mcp.tool_enabled(method) {
            return ControlResponse::error(format!(
                "the MCP tool '{method}' is disabled in Resolved settings"
            ));
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
        Ok(json!({
            "product": PRODUCT_NAME,
            "version": env!("CARGO_PKG_VERSION"),
            "protocol_version": crate::control_server::CONTROL_PROTOCOL_VERSION,
            "active_workspace": self.workspace_providers.active_id().to_string(),
            "workspace_writable": self.workspace_writable,
            "enabled_tools": self.settings.mcp.enabled_tools
        }))
    }

    fn control_list_workspaces(&self) -> Result<Value, String> {
        let active = self.workspace_providers.active_id().to_string();
        let workspaces = self
            .local_workspaces
            .iter()
            .map(|workspace| {
                let id = format!("local:{}", workspace.id);
                json!({
                    "id": id,
                    "name": workspace.name,
                    "active": id == active,
                    "writable": true
                })
            })
            .collect::<Vec<_>>();
        Ok(json!({ "workspaces": workspaces, "active_workspace": active }))
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
        self.commit_control_workspace(candidate, cx)?;
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

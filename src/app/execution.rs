use super::*;
use serde::Deserialize;
use std::sync::atomic::Ordering;

#[cfg(test)]
mod policy_scope_tests {
    use super::*;

    #[test]
    fn saved_request_scope_tracks_actual_folder_not_url_or_root_selection() {
        let mut workspace = crate::core::Workspace::default();
        let root = workspace.create_collection("Root").unwrap();
        let folder = workspace
            .create_collection_folder(&root, None, "Parent")
            .unwrap();
        let nested = workspace
            .create_collection_folder(&root, Some(&folder), "Nested")
            .unwrap();
        let template = RequestTemplate::new(RequestDraft::new("GET", "https://same.example"));
        let at_root = workspace
            .create_saved_request_in_folder(&root, None, "Root request", template.clone())
            .unwrap();
        let in_folder = workspace
            .create_saved_request_in_folder(&root, Some(&nested), "Nested request", template)
            .unwrap();
        assert_eq!(
            request_collection_scope(&workspace, Some(&at_root)).unwrap(),
            Some(root.clone())
        );
        assert_eq!(
            request_collection_scope(&workspace, Some(&in_folder)).unwrap(),
            Some(nested)
        );
        assert_eq!(request_collection_scope(&workspace, None).unwrap(), None);
        assert!(request_collection_scope(&workspace, Some("deleted")).is_err());
        workspace
            .move_saved_request(&root, &in_folder, Some(&folder))
            .unwrap();
        assert_eq!(
            request_collection_scope(&workspace, Some(&in_folder)).unwrap(),
            Some(folder)
        );
    }

    #[test]
    fn execution_snapshot_is_independent_of_later_policy_edits() {
        let mut limits: crate::core::execution_limits::ExecutionLimits = serde_json::from_value(
            serde_json::json!({"chain.max_requests": {"unlimited": false, "value": 0}}),
        )
        .unwrap();
        let snapshot = PreparedExecution {
            scope_key: "local:test".to_owned(),
            limits: limits.clone(),
            upstream: None,
        };
        limits = serde_json::from_value(
            serde_json::json!({"chain.max_requests": {"unlimited": true, "value": 0}}),
        )
        .unwrap();
        assert!(!snapshot.limits.get("chain.max_requests").unlimited);
        assert_eq!(snapshot.limits.get("chain.max_requests").value, 0);
        assert!(limits.get("chain.max_requests").unlimited);
    }
}

/// Immutable execution state captured before any user script runs.
#[derive(Clone)]
pub(super) struct PreparedExecution {
    pub(super) scope_key: String,
    pub(super) limits: crate::core::execution_limits::ExecutionLimits,
    pub(super) upstream: Option<PreparedUpstreamExecution>,
}

#[derive(Clone)]
pub(super) struct PreparedUpstreamExecution {
    pub(super) target: workspace_connections::ActiveUpstreamWorkspace,
    pub(super) credential: Arc<crate::core::UpstreamCredential>,
    /// Saved identity for proxy assignments, captured before scripts or tab changes.
    pub(super) saved_request_id: Option<String>,
    pub(super) collection_id: Option<String>,
    pub(super) policy: crate::core::ProxyExecutionPolicy,
}

/// Remote folder IDs are remote collection IDs. Never associate a draft by URL.
pub(super) fn request_collection_scope(
    workspace: &crate::core::Workspace,
    request_id: Option<&str>,
) -> Result<Option<String>, String> {
    let Some(id) = request_id else {
        return Ok(None);
    };
    let (collection, request) = workspace
        .saved_request(id)
        .ok_or_else(|| format!("Saved request '{id}' is no longer present."))?;
    Ok(Some(
        request.folder_id.as_ref().unwrap_or(&collection.id).clone(),
    ))
}

impl PreparedExecution {
    pub(super) async fn send(
        &self,
        upstream_client: &Client,
        request: RequestDraft,
        cookie_jar: &Arc<CookieJar>,
    ) -> Result<ResponseData, RequestError> {
        let local_client =
            crate::core::build_http_client_with_limits(cookie_jar.clone(), &self.limits)?;
        match &self.upstream {
            Some(upstream) => {
                crate::core::send_request_for_upstream_workspace_with_scope(
                    upstream_client,
                    &local_client,
                    &upstream.target.base_url,
                    upstream.credential.bearer_token(),
                    &upstream.target.workspace_id,
                    upstream.saved_request_id.as_deref(),
                    request,
                    cookie_jar,
                    upstream.collection_id.as_deref(),
                    &upstream.policy,
                )
                .await
            }
            None => {
                crate::core::send_request_with_limits(&local_client, request, &self.limits).await
            }
        }
    }
}

impl ApiTester {
    /// Capture identity synchronously, then authenticate and fetch before script execution.
    pub(super) fn prepare_execution(
        &self,
        request_id: Option<&str>,
    ) -> impl std::future::Future<Output = Result<PreparedExecution, String>> + Send + 'static + use<>
    {
        let collection_id = request_collection_scope(&self.workspace, request_id);
        let saved_request_id = request_id.map(str::to_owned);
        let provider_key = self.workspace_providers.active_id().to_string();
        let local = match self.workspace_providers.active_id() {
            WorkspaceProviderId::Local(_) => {
                Some(self.local_execution_limits_for_request(request_id))
            }
            WorkspaceProviderId::Upstream { .. } => None,
        };
        let target = self.active_upstream_workspace();
        let vault = self.credential_vault.clone();
        let runtime = self.runtime.clone();
        let client = self.upstream_execution_client.clone();
        async move {
            let collection_id = collection_id?;
            let scope_key = serde_json::json!([provider_key, collection_id]).to_string();
            if let Some(local) = local {
                return Ok(PreparedExecution {
                    scope_key,
                    limits: local?,
                    upstream: None,
                });
            }
            let target = target?;
            let upstream_id = target.upstream_id.clone();
            let credential = runtime
                .spawn_blocking(move || vault.load_upstream(&upstream_id))
                .await
                .map_err(|error| error.to_string())?
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "Log in to this server again.".to_owned())?;
            if credential.expires_at <= Utc::now() {
                return Err("Log in to this server again.".to_owned());
            }
            let policy = crate::core::get_upstream_execution_policy_for_scope(
                &client,
                &target.base_url,
                credential.bearer_token(),
                &target.workspace_id,
                collection_id.as_deref(),
            )
            .await
            .map_err(|error| error.to_string())?;
            Ok(PreparedExecution {
                scope_key,
                limits: policy.limits.clone(),
                upstream: Some(PreparedUpstreamExecution {
                    target,
                    credential: Arc::new(credential),
                    saved_request_id,
                    collection_id,
                    policy,
                }),
            })
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub(super) struct McpHttpRequestOverrides {
    pub method: Option<String>,
    pub url: Option<String>,
    pub query_parameters: Option<Vec<QueryParamEntry>>,
    pub headers: Option<Vec<HeaderEntry>>,
    pub body: Option<String>,
    pub body_mode: Option<BodyMode>,
    pub raw_body_language: Option<RawBodyLanguage>,
    pub body_fields: Option<Vec<BodyField>>,
}

impl McpHttpRequestOverrides {
    fn apply(self, request: &mut RequestDraft) {
        if let Some(method) = self.method {
            request.method = method;
        }
        if let Some(url) = self.url {
            request.url = url;
            request.query_params = query_params_from_url(&request.url);
        }
        if let Some(query_parameters) = self.query_parameters {
            request.url = url_with_query_params(&request.url, &query_parameters);
            request.query_params = query_parameters;
        }
        if let Some(headers) = self.headers {
            request.headers = headers;
        }
        if let Some(body) = self.body {
            request.body = body;
        }
        if let Some(body_mode) = self.body_mode {
            request.body_mode = body_mode;
        }
        if let Some(raw_body_language) = self.raw_body_language {
            request.raw_body_language = raw_body_language;
        }
        if let Some(body_fields) = self.body_fields {
            request.body_fields = body_fields;
        }
    }
}

// Script errors intentionally carry a full structured report and diagnostic.
// Keeping the unboxed error preserves that context across the background task.
#[allow(clippy::result_large_err)]
impl ApiTester {
    pub(super) fn script_scope(environment: Option<&Environment>) -> ScriptScope {
        let mut script_environment = ScriptEnvironment::default();
        if let Some(environment) = environment {
            for variable in environment
                .variables
                .iter()
                .filter(|variable| variable.enabled)
            {
                if variable.secret {
                    script_environment.insert_secret(variable.key.clone(), variable.value.clone());
                } else {
                    script_environment.insert(variable.key.clone(), variable.value.clone());
                }
            }
        }
        ScriptScope {
            environment: script_environment,
            ..ScriptScope::default()
        }
    }

    pub(super) fn start_request(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sending {
            return;
        }

        let validation_error = if input_text_is_blank(&self.method, cx) {
            Some("HTTP method cannot be empty.".to_owned())
        } else if self.active_environment_editor_is_dirty(cx) {
            Some(
                "The active environment has unsaved changes. Save or Revert them before sending."
                    .to_owned(),
            )
        } else {
            None
        };
        if let Some(error) = validation_error {
            self.response = None;
            self.response_request = None;
            self.response_sensitive_values.clear();
            self.request_error = Some(error);
            self.script_diagnostic = None;
            self.execution_stage = None;
            self.hide_preview(cx);
            cx.notify();
            return;
        }

        self.dismiss_template_variable_popover();
        self.request_notice = None;
        self.mcp_script_console_operation_id = None;
        self.mcp_script_console_request_id = None;
        let template = self.request_template(cx);
        self.begin_request_template(template, window, cx);
    }

    pub(super) fn start_control_http_request(
        &mut self,
        request_id: &str,
        overrides: Option<McpHttpRequestOverrides>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<u64, String> {
        if self.sending || self.script_console_running {
            return Err(
                "Another request or script-console evaluation is already running.".to_owned(),
            );
        }
        if self.active_environment_editor_is_dirty(cx) {
            return Err(
                "The active environment has unsaved changes. Save or Revert them before executing through MCP."
                    .to_owned(),
            );
        }
        let (collection, request) = self
            .workspace
            .saved_request(request_id)
            .ok_or_else(|| format!("request '{request_id}' was not found"))?;
        let collection_id = collection.id.clone();
        if request.definition.is_websocket() {
            return Err(format!(
                "request '{request_id}' is a WebSocket document; use connect_websocket"
            ));
        }
        let mut template = request.definition.clone();
        if let Some(overrides) = overrides {
            overrides.apply(&mut template.request);
        }
        if template.request.method.trim().is_empty() {
            return Err("HTTP method cannot be empty.".to_owned());
        }
        self.dismiss_template_variable_popover();
        self.request_notice = None;
        self.open_saved_request_tab(collection_id, request_id.to_owned(), window, cx);
        let operation_id = self.begin_request_template(template, window, cx);
        self.mcp_http_operation_id = Some(operation_id);
        self.mcp_http_request_id = Some(request_id.to_owned());
        self.mcp_http_exchange = None;
        Ok(operation_id)
    }

    pub(super) fn begin_request_template(
        &mut self,
        template: RequestTemplate,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> u64 {
        let environment_id = self.workspace.active_environment_id.clone();
        let mut scope = Self::script_scope(
            environment_id
                .as_deref()
                .and_then(|id| self.workspace.environment(id)),
        );
        scope.script_timeout = self.settings.script.timeout();
        self.script_console_session = None;
        self.script_console_session_key = None;
        self.script_console_reports.clear();
        self.script_console_hidden_rows = 0;
        self.request_generation = self.request_generation.wrapping_add(1);
        let generation = self.request_generation;
        self.request_history_target = self.active_upstream_workspace().ok();
        self.prepared_execution = None;
        let preparation = self.prepare_execution(self.active_saved_request_id.as_deref());
        self.sending = true;
        self.execution_stage = Some(ExecutionStage::PreRequest);
        self.response = None;
        self.response_request = None;
        self.response_sensitive_values.clear();
        self.request_error = None;
        self.script_diagnostic = None;
        self.pre_script_report = None;
        self.post_script_report = None;
        self.preview_error = None;
        self.copied = false;
        self.hide_preview(cx);

        let source = template.scripts.pre_request.clone();
        let request = template.request.clone();
        let namespace = self.request_namespace.clone();
        let cancellation = ScriptCancellation::new();
        self.script_cancellation = Some(cancellation.clone());
        self.chain_budget.store(0, Ordering::Relaxed);
        let tape_environment = environment_id.clone();
        let chainer = self.build_inline_chainer(&tape_environment);
        let runtime = self.runtime.clone();
        let task = self.runtime.spawn(async move {
            let prepared = preparation.await?;
            scope.execution_limits = Some(prepared.limits.clone());
            let chainer = chainer.with_execution_limits(&prepared.limits);
            let result = runtime
                .spawn_blocking(move || {
                    crate::core::execute_pre_request_with_chain(
                        &source,
                        &request,
                        &scope,
                        &namespace,
                        &cancellation,
                        Some(&chainer),
                    )
                })
                .await;
            Ok::<_, String>((prepared, result))
        });
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if generation != this.request_generation {
                    return;
                }
                match result {
                    Ok(Ok((prepared, result))) => {
                        this.prepared_execution = Some(prepared);
                        this.finish_pre_request(
                            generation,
                            template,
                            environment_id,
                            result,
                            window,
                            cx,
                        );
                    }
                    Ok(Err(error)) => this.fail_request(&template.request, error, cx),
                    Err(error) => this.fail_request(&template.request, error.to_string(), cx),
                }
            });
        })
        .detach();
        generation
    }

    pub(super) fn finish_pre_request(
        &mut self,
        generation: u64,
        template: RequestTemplate,
        environment_id: Option<String>,
        result: Result<Result<PreRequestResult, ScriptError>, tokio::task::JoinError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if generation != self.request_generation {
            return;
        }

        let result = match result {
            Ok(result) => result,
            Err(error) => {
                self.fail_request(
                    &template.request,
                    format!("pre-request script task failed: {error}"),
                    cx,
                );
                return;
            }
        };
        let pre_result = match result {
            Ok(result) => result,
            Err(error) => {
                if matches!(
                    error.diagnostic.kind,
                    crate::core::ScriptErrorKind::Cancelled
                ) {
                    self.finish_cancelled(cx);
                    return;
                }
                self.pre_script_report = Some(error.report.clone());
                self.script_diagnostic = Some(error.diagnostic.clone());
                self.response_tab = ResponseTab::Scripts;
                self.fail_request(&template.request, error.to_string(), cx);
                return;
            }
        };

        self.pre_script_report = Some(pre_result.report.clone());
        if let Err(error) = self.apply_environment_mutations(
            environment_id.as_deref(),
            &pre_result.environment_mutations,
            window,
            cx,
        ) {
            self.fail_request(&template.request, error, cx);
            return;
        }

        if pre_result.chained_requests.is_empty() {
            self.finish_pre_chain_send(
                generation,
                template,
                environment_id,
                pre_result,
                window,
                cx,
            );
        } else {
            self.run_pre_chain(generation, template, environment_id, pre_result, window, cx);
        }
    }

    /// Resolve and send the parent request using the now-current environment,
    /// after any pre-request chain has run.
    #[allow(clippy::too_many_arguments)]
    fn finish_pre_chain_send(
        &mut self,
        generation: u64,
        template: RequestTemplate,
        environment_id: Option<String>,
        pre_result: PreRequestResult,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let environment = environment_id
            .as_deref()
            .and_then(|id| self.workspace.environment(id));
        let mut resolved = match resolve_request(&pre_result.request, environment) {
            Ok(resolved) => resolved,
            Err(error) => {
                self.fail_request(&template.request, error.to_string(), cx);
                return;
            }
        };
        if let Some(environment) = environment {
            resolved.sensitive_values.extend(
                environment
                    .variables
                    .iter()
                    .filter(|variable| variable.enabled && variable.secret)
                    .map(|variable| variable.value.clone()),
            );
        }
        self.begin_network_request(
            generation,
            template,
            environment_id,
            resolved,
            pre_result.report,
            window,
            cx,
        );
    }

    /// Schedule and run a pre-request chain. If any chained request fails, the
    /// parent is not sent.
    #[allow(clippy::too_many_arguments)]
    fn run_pre_chain(
        &mut self,
        generation: u64,
        template: RequestTemplate,
        environment_id: Option<String>,
        pre_result: PreRequestResult,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let scheduled = pre_result.chained_requests.clone();
        let completion_environment = environment_id.clone();
        self.spawn_chain(
            generation,
            environment_id,
            scheduled,
            self.prepared_execution
                .as_ref()
                .map(|prepared| prepared.limits.clone()),
            window,
            cx,
            move |this, run, window, cx| {
                this.apply_chain_run(&run, window, cx);
                if let Some(failure) = run.error {
                    let message = this.chain_failure_message(&failure);
                    this.fail_request(&template.request, message, cx);
                    return;
                }
                this.finish_pre_chain_send(
                    generation,
                    template,
                    completion_environment,
                    pre_result,
                    window,
                    cx,
                );
            },
        );
    }

    /// Spawn the shared recursive chain runner for the active (local or
    /// upstream) workspace, then invoke `on_complete` on the main thread with
    /// the chain outcome. The whole chain shares one cancellation token and one
    /// execution budget with the top-level Send.
    fn spawn_chain(
        &mut self,
        generation: u64,
        environment_id: Option<String>,
        scheduled: Vec<crate::core::ChainedRequest>,
        execution_limits: Option<crate::core::execution_limits::ExecutionLimits>,
        window: &mut Window,
        cx: &mut Context<Self>,
        on_complete: impl FnOnce(&mut Self, crate::core::ChainRun, &mut Window, &mut Context<Self>)
        + Send
        + 'static,
    ) {
        let workspace = self.workspace.clone();
        let namespace = self.request_namespace.clone();
        let Some(chain_cancellation) = self.script_cancellation.clone() else {
            self.fail_request(
                &crate::core::RequestDraft::new("GET", ""),
                "Request execution state was lost; please try again.".to_owned(),
                cx,
            );
            return;
        };
        let budget = Arc::clone(&self.chain_budget);
        let runner = self.build_inline_chainer(&environment_id);
        let runner = match execution_limits {
            Some(limits) => runner.with_execution_limits(&limits),
            None => runner,
        };
        let limits = runner.limits;

        let task = self.runtime.spawn(async move {
            crate::core::run_chain_with_executor(
                &workspace,
                environment_id.as_deref(),
                &scheduled,
                &namespace,
                &chain_cancellation,
                Some(&runner),
                &runner,
                limits,
                &budget,
            )
            .await
        });

        cx.spawn_in(window, async move |this, cx| {
            let run = match task.await {
                Ok(run) => run,
                Err(error) => crate::core::ChainRun {
                    error: Some(crate::core::ChainFailure {
                        path: None,
                        message: format!("Chained execution task failed: {error}"),
                    }),
                    ..crate::core::ChainRun::default()
                },
            };
            let _ = this.update_in(cx, |this, window, cx| {
                if generation != this.request_generation {
                    return;
                }
                on_complete(this, run, window, cx);
            });
        })
        .detach();
    }

    /// Builds an inline chainer for awaited `api.requests.execute(...)`. Returns
    /// a self-referential runner ([`InlineChainRunner`]): invoked by the script
    /// pump, it drives the whole awaited batch of pipelines concurrently via
    /// `join_all` on the shared Tokio runtime — driven from a fresh scoped
    /// thread so a chained request's own `await execute(...)` can re-enter the
    /// runner without a "runtime within a runtime" panic — and hands *itself*
    /// to `run_chain` so chained scripts can await further. Each pipeline is
    /// bounded by the script timeout. Returns one `Result` per request, in
    /// input order; `Err` on chain failure so the awaiting script rejects.
    pub(super) fn build_inline_chainer(
        &self,
        environment_id: &Option<String>,
    ) -> InlineChainRunner {
        let workspace = self.workspace.clone();
        let namespace = self.request_namespace.clone();
        let environment_id = environment_id.clone();
        let chain_cancellation = self.script_cancellation.clone().unwrap_or_default();
        let budget = Arc::clone(&self.chain_budget);
        let cookie_jar = self.cookie_jar.clone();
        let upstream_client = self.upstream_execution_client.clone();
        let vault = self.credential_vault.clone();
        let runtime = Arc::clone(&self.runtime);
        let target = match self.workspace_providers.active_id() {
            WorkspaceProviderId::Local(_) => Ok(None),
            WorkspaceProviderId::Upstream { .. } => self.active_upstream_workspace().map(Some),
        };
        let local_limits = workspace
            .collections
            .iter()
            .flat_map(|collection| collection.requests.iter())
            .map(|request| {
                (
                    request.id.clone(),
                    self.local_execution_limits_for_request(Some(&request.id)),
                )
            })
            .collect();
        let limits = crate::core::ChainLimits {
            script_timeout: self.settings.script.timeout(),
            ..Default::default()
        };
        let runner = InlineChainRunner {
            provider_key: self.workspace_providers.active_id().to_string(),
            workspace,
            namespace,
            environment_id,
            chain_cancellation,
            budget,
            cookie_jar,
            upstream_client,
            vault,
            runtime,
            target,
            local_limits,
            limits,
        };
        if let Some(prepared) = &self.prepared_execution {
            runner.with_execution_limits(&prepared.limits)
        } else {
            runner
        }
    }

    /// Apply the effects of a finished chain to the real workspace: persist the
    /// chained requests' environment mutations and record their history entries
    /// and shared-history uploads.
    fn apply_chain_run(
        &mut self,
        run: &crate::core::ChainRun,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !run.environment_mutations.is_empty()
            && let Some(environment_id) = self.workspace.active_environment_id.clone()
        {
            let _ = self.apply_environment_mutations(
                Some(&environment_id),
                &run.environment_mutations,
                window,
                cx,
            );
        }
        for entry in &run.history {
            self.history.push(entry.clone());
        }
        for shared in &run.shared_history {
            self.upload_shared_history_entry(shared.clone(), cx);
        }
    }

    /// Build a secrets-redacted message for a chain failure.
    fn chain_failure_message(&self, failure: &crate::core::ChainFailure) -> String {
        let redactor = Self::script_scope(self.workspace.active_environment()).redactor();
        let message = if let Some(path) = &failure.path {
            format!("Chained request '{path}' failed: {}", failure.message)
        } else {
            format!("Chained execution failed: {}", failure.message)
        };
        redactor.scrub(&message)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn begin_network_request(
        &mut self,
        generation: u64,
        template: RequestTemplate,
        environment_id: Option<String>,
        resolved: crate::core::ResolvedRequest,
        pre_report: ScriptReport,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execution_stage = Some(ExecutionStage::Request);
        let Some(prepared) = self.prepared_execution.clone() else {
            self.fail_request(
                &resolved.request,
                "Execution policy snapshot was lost.".to_owned(),
                cx,
            );
            return;
        };
        let client = self.upstream_execution_client.clone();
        let cookie_jar = self.cookie_jar.clone();
        let request = resolved.request.clone();
        let task = RequestTask::spawn(self.runtime.handle(), async move {
            prepared.send(&client, request, &cookie_jar).await
        });
        self.abort_handle = Some(task.abort_handle());
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let result = task.wait().await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.finish_network_request(
                    generation,
                    template,
                    environment_id,
                    resolved,
                    pre_report,
                    result,
                    window,
                    cx,
                );
            });
        })
        .detach();
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn finish_network_request(
        &mut self,
        generation: u64,
        template: RequestTemplate,
        environment_id: Option<String>,
        resolved: crate::core::ResolvedRequest,
        pre_report: ScriptReport,
        result: Result<ResponseData, RequestError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if generation != self.request_generation {
            return;
        }
        self.abort_handle = None;

        let response = match result {
            Ok(response) => response,
            Err(RequestError::Cancelled) => {
                self.finish_cancelled(cx);
                return;
            }
            Err(RequestError::ProxyDestinationBlocked {
                request,
                address,
                reason,
            }) => {
                self.pre_script_report = Some(pre_report);
                let allowlist_request = request.clone();
                let display_request = resolved.redact_secrets(&request);
                let message = format!(
                    "Proxy blocked {display_request} because {address} is not allowed ({reason})."
                );
                self.show_proxy_destination_blocked(
                    allowlist_request,
                    address,
                    message.clone(),
                    window,
                    cx,
                );
                self.fail_request_with_secrets(
                    &resolved.request,
                    message,
                    &resolved.sensitive_values,
                    cx,
                );
                return;
            }
            Err(error) => {
                self.pre_script_report = Some(pre_report);
                self.fail_request_with_secrets(
                    &resolved.request,
                    resolved.redact_secrets(&error.to_string()),
                    &resolved.sensitive_values,
                    cx,
                );
                return;
            }
        };

        let mut display_response = response.clone();
        display_response.final_url = resolved.redact_secrets(&response.final_url);
        self.response = Some(display_response.clone());
        self.response_request = Some(resolved.request.clone());
        self.response_sensitive_values = resolved.sensitive_values.clone();
        self.update_response_editor(&display_response, window, cx);

        self.execution_stage = Some(ExecutionStage::PostResponse);
        let mut scope = Self::script_scope(
            environment_id
                .as_deref()
                .and_then(|id| self.workspace.environment(id)),
        );
        scope.script_timeout = self.settings.script.timeout();
        scope.execution_limits = self
            .prepared_execution
            .as_ref()
            .map(|prepared| prepared.limits.clone());
        let source = template.scripts.post_response.clone();
        let request = resolved.request.clone();
        let history_request = resolved.request.clone();
        let history_sensitive_values = resolved.sensitive_values.clone();
        let namespace = self.request_namespace.clone();
        let cancellation = self
            .script_cancellation
            .clone()
            .expect("chain cancellation is present while sending");
        self.script_cancellation = Some(cancellation.clone());
        let tape_environment = environment_id.clone();
        let chainer = self.build_inline_chainer(&tape_environment);
        let task = self.runtime.spawn_blocking(move || {
            crate::core::execute_post_response_with_chain(
                &source,
                &request,
                &response,
                &scope,
                &namespace,
                &cancellation,
                Some(&chainer),
            )
        });
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.finish_post_response(
                    generation,
                    environment_id,
                    pre_report,
                    display_response,
                    history_request,
                    history_sensitive_values,
                    result,
                    window,
                    cx,
                );
            });
        })
        .detach();
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn finish_post_response(
        &mut self,
        generation: u64,
        environment_id: Option<String>,
        pre_report: ScriptReport,
        response: ResponseData,
        history_request: RequestDraft,
        history_sensitive_values: Vec<String>,
        result: Result<Result<PostResponseResult, ScriptError>, tokio::task::JoinError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if generation != self.request_generation {
            return;
        }

        self.execution_stage = None;
        self.pre_script_report = Some(pre_report);
        self.response = Some(response.clone());

        let mut post_chained = Vec::new();
        match result {
            Ok(Ok(post_result)) => {
                post_chained = post_result.chained_requests;
                let mutation_result = self.apply_environment_mutations(
                    environment_id.as_deref(),
                    &post_result.environment_mutations,
                    window,
                    cx,
                );
                self.post_script_report = Some(post_result.report);
                match mutation_result {
                    Ok(()) => {
                        self.script_diagnostic = None;
                        self.request_error = None;
                    }
                    Err(error) => {
                        self.request_error = Some(error);
                        self.response_tab = ResponseTab::Scripts;
                        self.hide_preview(cx);
                    }
                }
            }
            Ok(Err(error)) => {
                if matches!(
                    error.diagnostic.kind,
                    crate::core::ScriptErrorKind::Cancelled
                ) {
                    self.request_error =
                        Some("Request completed; post-response script cancelled".to_owned());
                } else {
                    self.request_error = Some(error.to_string());
                }
                self.post_script_report = Some(error.report);
                self.script_diagnostic = Some(error.diagnostic);
                self.response_tab = ResponseTab::Scripts;
                self.hide_preview(cx);
            }
            Err(error) => {
                self.request_error = Some(format!("post-response script task failed: {error}"));
                self.response_tab = ResponseTab::Scripts;
                self.hide_preview(cx);
            }
        }

        // The parent exchange completed; record its history entry before running
        // any post-response chain so the response is preserved.
        let history_entry = HistoryEntry::completed_with_secrets(
            &history_request,
            &response,
            &history_sensitive_values,
        );
        let shared_history = SharedHistoryUpload::completed(
            history_entry.id.clone(),
            history_entry.created_at,
            &history_request,
            &response,
            &history_sensitive_values,
        );
        self.history.push(history_entry);
        self.upload_shared_history_entry(shared_history, cx);

        if post_chained.is_empty() {
            self.finalize_post_response(generation, window, cx);
        } else {
            let limits = self
                .prepared_execution
                .as_ref()
                .map(|prepared| prepared.limits.clone());
            self.run_post_chain(generation, environment_id, post_chained, limits, window, cx);
        }
    }

    /// Finalize the parent request's completion and reveal its response. Runs
    /// only after the parent's post-response chain (if any) has finished.
    fn finalize_post_response(
        &mut self,
        generation: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if generation != self.request_generation {
            return;
        }
        self.sending = false;
        self.script_cancellation = None;
        self.persist_history();
        if self.response_tab == ResponseTab::Preview
            && self.workspace_tabs.active() == ActiveWorkspaceTab::Request
            && self.sidebar_tab != SidebarTab::Environments
        {
            self.show_preview(window, cx);
        } else {
            self.hide_preview(cx);
        }
        self.capture_mcp_http_exchange(generation);
        self.advance_mcp_request_sequence(window, cx);
        cx.notify();
    }

    pub(super) fn run_post_chain(
        &mut self,
        generation: u64,
        environment_id: Option<String>,
        post_chained: Vec<crate::core::ChainedRequest>,
        execution_limits: Option<crate::core::execution_limits::ExecutionLimits>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.spawn_chain(
            generation,
            environment_id,
            post_chained,
            execution_limits,
            window,
            cx,
            move |this, run, window, cx| {
                this.apply_chain_run(&run, window, cx);
                // The parent's already-received response must remain; surface a
                // chained-request failure in the existing diagnostics UI.
                if let Some(failure) = run.error {
                    let message = this.chain_failure_message(&failure);
                    this.request_error = Some(message);
                    this.response_tab = ResponseTab::Scripts;
                    this.hide_preview(cx);
                }
                this.finalize_post_response(generation, window, cx);
            },
        );
    }

    pub(super) fn cancel_request(&mut self, cx: &mut Context<Self>) {
        if let Some(cancellation) = self.script_cancellation.take() {
            cancellation.cancel();
        }
        if let Some(abort_handle) = self.abort_handle.take() {
            abort_handle.abort();
        }
        if self.response.is_some() && self.execution_stage == Some(ExecutionStage::PostResponse) {
            // The network request already completed and the response is shown; only the
            // post-response script is pending. Letting `finish_post_response` observe the
            // Cancelled script error records the completed history entry (labeled
            // "post-response script cancelled") instead of relabeling the received response
            // as "Request cancelled" and silently dropping the history entry.
            return;
        }
        let mcp_operation_id = (self.mcp_http_operation_id == Some(self.request_generation))
            .then_some(self.request_generation);
        self.script_console_session = None;
        self.script_console_session_key = None;
        self.script_console_reports.clear();
        self.script_console_hidden_rows = 0;
        self.request_generation = self.request_generation.wrapping_add(1);
        self.finish_cancelled(cx);
        if let Some(operation_id) = mcp_operation_id {
            self.capture_mcp_http_exchange(operation_id);
        }
    }

    pub(super) fn finish_cancelled(&mut self, cx: &mut Context<Self>) {
        self.sending = false;
        self.execution_stage = None;
        self.abort_handle = None;
        self.script_cancellation = None;
        self.request_history_target = None;
        self.request_error = Some("Request cancelled".to_owned());
        self.preview_error = None;
        self.hide_preview(cx);
        self.capture_mcp_http_exchange(self.request_generation);
        cx.notify();
    }

    pub(super) fn fail_request(
        &mut self,
        request: &RequestDraft,
        message: String,
        cx: &mut Context<Self>,
    ) {
        self.fail_request_with_secrets(request, message, &[], cx);
    }

    pub(super) fn fail_request_with_secrets(
        &mut self,
        request: &RequestDraft,
        message: String,
        sensitive_values: &[String],
        cx: &mut Context<Self>,
    ) {
        self.sending = false;
        self.execution_stage = None;
        self.abort_handle = None;
        self.script_cancellation = None;
        let history_entry =
            HistoryEntry::failed_with_secrets(request, message.clone(), sensitive_values);
        let shared_history = SharedHistoryUpload::failed(
            history_entry.id.clone(),
            history_entry.created_at,
            request,
            &message,
            sensitive_values,
        );
        self.history.push(history_entry);
        self.request_error = Some(message);
        self.persist_history();
        self.upload_shared_history_entry(shared_history, cx);
        self.capture_mcp_http_exchange(self.request_generation);
        cx.notify();
    }

    fn show_proxy_destination_blocked(
        &mut self,
        request: String,
        address: String,
        message: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let can_allowlist = self.active_upstream_has_permission(SERVER_SETTINGS_UPDATE);
        let mut notification = Notification::error(message)
            .title("Proxy destination blocked")
            .autohide(false);
        if can_allowlist {
            let owner = cx.entity().downgrade();
            notification = notification.content(move |_, _, _| {
                let request_owner = owner.clone();
                let request_value = request.clone();
                let address_owner = owner.clone();
                let address_value = address.clone();
                h_flex()
                    .mt_2()
                    .gap_2()
                    .child(
                        Button::new("allow-proxy-request")
                            .label("Allow this request")
                            .small()
                            .link()
                            .on_click(move |_, window, cx| {
                                let _ = request_owner.update(cx, |this, cx| {
                                    this.allowlist_proxy_destination(
                                        "request",
                                        request_value.clone(),
                                        window,
                                        cx,
                                    );
                                });
                            }),
                    )
                    .child(
                        Button::new("allow-proxy-address")
                            .label("Allow this address")
                            .small()
                            .link()
                            .on_click(move |_, window, cx| {
                                let _ = address_owner.update(cx, |this, cx| {
                                    this.allowlist_proxy_destination(
                                        "address",
                                        address_value.clone(),
                                        window,
                                        cx,
                                    );
                                });
                            }),
                    )
                    .into_any_element()
            });
        }
        window.push_notification(notification, cx);
    }

    fn allowlist_proxy_destination(
        &mut self,
        kind: &'static str,
        value: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.active_upstream_has_permission(SERVER_SETTINGS_UPDATE) {
            window.push_notification(
                Notification::error("You do not have permission to change the proxy allowlist."),
                cx,
            );
            return;
        }
        let target = match self.active_upstream_workspace() {
            Ok(target) => target,
            Err(error) => {
                window.push_notification(Notification::error(error), cx);
                return;
            }
        };
        let vault = self.credential_vault.clone();
        let client = self.upstream_client.clone();
        let runtime = Arc::clone(&self.runtime);
        let upstream_id = target.upstream_id.clone();
        let success_message = if kind == "request" {
            "Allowed this proxy request.".to_owned()
        } else {
            format!("Allowed proxy address: {value}")
        };
        let task_value = value.clone();
        let task = self.runtime.spawn(async move {
            let credential = runtime
                .spawn_blocking(move || vault.load_upstream(&upstream_id))
                .await
                .map_err(|error| error.to_string())?
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "Log in to this server again.".to_owned())?;
            add_upstream_proxy_allowlist_entry(
                &client,
                &target.base_url,
                credential.bearer_token(),
                kind,
                &task_value,
            )
            .await
            .map_err(|error| error.to_string())
        });
        cx.spawn_in(window, async move |_, cx| {
            let result = task.await;
            let _ = cx.update(|window, cx| match result {
                Ok(Ok(())) => window.push_notification(Notification::success(success_message), cx),
                Ok(Err(error)) => window.push_notification(
                    Notification::error(format!("Could not update the proxy allowlist: {error}")),
                    cx,
                ),
                Err(error) => window.push_notification(
                    Notification::error(format!("Could not update the proxy allowlist: {error}")),
                    cx,
                ),
            });
        })
        .detach();
    }

    pub(super) fn apply_environment_mutations(
        &mut self,
        environment_id: Option<&str>,
        mutations: &[EnvironmentMutation],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if mutations.is_empty() {
            return Ok(());
        }

        let Some(environment_id) = environment_id else {
            self.workspace_warning = Some(
                "Script environment changes were transient because no environment is active."
                    .to_owned(),
            );
            return Ok(());
        };
        if !self.workspace_writable {
            return self.apply_environment_mutations_on_upstream(
                environment_id,
                mutations,
                window,
                cx,
            );
        }

        let mut candidate = self.workspace.clone();
        crate::core::apply_environment_mutations_to_workspace(
            &mut candidate,
            environment_id,
            mutations,
        )?;
        self.commit_workspace(candidate)
            .map_err(|error| format!("Script environment update was not saved: {error}"))?;
        self.refresh_variable_intelligence(cx);
        if self.selected_environment_id.as_deref() == Some(environment_id) {
            self.reload_environment_editor(window, cx);
        }
        Ok(())
    }

    /// Persist script environment changes on a remote workspace. The new value
    /// is exposed locally immediately so the running request can use it, while
    /// the write is pushed back to the server in the background so a slow or
    /// failing write never blocks request execution.
    fn apply_environment_mutations_on_upstream(
        &mut self,
        environment_id: &str,
        mutations: &[EnvironmentMutation],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if !self.can_update_environment_values_content() {
            return Err(
                "You do not have permission to update environment values on this server."
                    .to_owned(),
            );
        }
        let WorkspaceProviderId::Upstream {
            upstream_id,
            workspace_id,
        } = self.workspace_providers.active_id()
        else {
            return Err("No server workspace is selected.".to_owned());
        };
        let upstream_id = upstream_id.clone();
        let workspace_id = workspace_id.clone();
        let base_url = self
            .settings
            .upstreams
            .server(&upstream_id)
            .and_then(|profile| profile.parsed_base_url())
            .ok_or_else(|| "That server URL is invalid.".to_owned())?;

        let baseline = self
            .workspace
            .environment(environment_id)
            .ok_or_else(|| format!("active environment '{environment_id}' no longer exists"))?
            .clone();
        let mut candidate = self.workspace.clone();
        crate::core::apply_environment_mutations_to_workspace(
            &mut candidate,
            environment_id,
            mutations,
        )?;
        let draft = candidate
            .environment(environment_id)
            .ok_or_else(|| format!("active environment '{environment_id}' no longer exists"))?
            .clone();

        // Reflect the mutation in the in-memory view (and provider cache) right
        // away, so pre-request-set values are visible to the request itself.
        self.replace_active_remote_workspace(candidate);
        self.refresh_variable_intelligence(cx);
        if self.selected_environment_id.as_deref() == Some(environment_id) {
            self.reload_environment_editor(window, cx);
        }

        let vault = self.credential_vault.clone();
        let client = self.upstream_client.clone();
        let runtime = Arc::clone(&self.runtime);
        let task = self.runtime.spawn(async move {
            let credential = runtime
                .spawn_blocking(move || vault.load_upstream(&upstream_id))
                .await
                .map_err(|error| format!("Could not open the saved session: {error}"))?
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "Log in to this server again.".to_owned())?;
            if credential.expires_at <= Utc::now() {
                return Err("Log in to this server again.".to_owned());
            }
            save_upstream_environment(
                &client,
                &base_url,
                credential.bearer_token(),
                &workspace_id,
                &baseline,
                &draft,
            )
            .await
            .map_err(|error| error.to_string())?;
            Ok(())
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, _, cx| {
                if let Err(error) = result {
                    this.workspace_warning = Some(format!(
                        "Environment changes were applied locally but could not be saved to the server: {error}"
                    ));
                }
                cx.notify();
            });
        })
        .detach();
        Ok(())
    }
}

#[derive(Clone)]
pub(super) struct InlineChainRunner {
    provider_key: String,
    workspace: crate::core::Workspace,
    namespace: crate::core::RequestNamespaceCatalog,
    environment_id: Option<String>,
    chain_cancellation: crate::core::ScriptCancellation,
    budget: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    cookie_jar: Arc<CookieJar>,
    upstream_client: reqwest::Client,
    vault: crate::core::CredentialVault,
    runtime: std::sync::Arc<tokio::runtime::Runtime>,
    target: Result<Option<workspace_connections::ActiveUpstreamWorkspace>, String>,
    local_limits: HashMap<String, Result<crate::core::execution_limits::ExecutionLimits, String>>,
    limits: crate::core::ChainLimits,
}

struct PreparedChainExecution<'a> {
    prepared: PreparedExecution,
    runner: &'a InlineChainRunner,
}

impl crate::core::ChainRequestExecution for PreparedChainExecution<'_> {
    fn execution_limits(&self) -> Option<&crate::core::execution_limits::ExecutionLimits> {
        Some(&self.prepared.limits)
    }
    fn send(
        &self,
        request: RequestDraft,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ResponseData, RequestError>> + Send + '_>,
    > {
        Box::pin(self.prepared.send(
            &self.runner.upstream_client,
            request,
            &self.runner.cookie_jar,
        ))
    }
}

impl crate::core::ChainExecutor for InlineChainRunner {
    fn prepare(
        &self,
        request_id: &str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<Box<dyn crate::core::ChainRequestExecution + '_>, RequestError>,
                > + Send
                + '_,
        >,
    > {
        let collection_id = request_collection_scope(&self.workspace, Some(request_id));
        let saved_request_id = request_id.to_owned();
        let local_limits = self.local_limits.get(request_id).cloned();
        Box::pin(async move {
            let collection_id = collection_id.map_err(RequestError::Upstream)?;
            let scope_key = serde_json::json!([self.provider_key, collection_id]).to_string();
            let prepared = match self.target.clone().map_err(RequestError::Upstream)? {
                None => PreparedExecution {
                    scope_key,
                    limits: local_limits
                        .ok_or_else(|| {
                            RequestError::Upstream("Saved request policy was lost.".to_owned())
                        })?
                        .map_err(RequestError::Upstream)?,
                    upstream: None,
                },
                Some(target) => {
                    let vault = self.vault.clone();
                    let upstream_id = target.upstream_id.clone();
                    let credential = self
                        .runtime
                        .spawn_blocking(move || vault.load_upstream(&upstream_id))
                        .await
                        .map_err(|error| RequestError::TaskFailed(error.to_string()))?
                        .map_err(|error| RequestError::Upstream(error.to_string()))?
                        .ok_or_else(|| {
                            RequestError::Upstream("Log in to this server again.".to_owned())
                        })?;
                    if credential.expires_at <= Utc::now() {
                        return Err(RequestError::Upstream(
                            "Log in to this server again.".to_owned(),
                        ));
                    }
                    let policy = crate::core::get_upstream_execution_policy_for_scope(
                        &self.upstream_client,
                        &target.base_url,
                        credential.bearer_token(),
                        &target.workspace_id,
                        collection_id.as_deref(),
                    )
                    .await?;
                    PreparedExecution {
                        scope_key,
                        limits: policy.limits.clone(),
                        upstream: Some(PreparedUpstreamExecution {
                            target,
                            credential: Arc::new(credential),
                            saved_request_id: Some(saved_request_id),
                            collection_id,
                            policy,
                        }),
                    }
                }
            };
            Ok(Box::new(PreparedChainExecution {
                prepared,
                runner: self,
            })
                as Box<dyn crate::core::ChainRequestExecution>)
        })
    }
}

impl InlineChainRunner {
    pub(super) fn with_execution_limits(
        mut self,
        limits: &crate::core::execution_limits::ExecutionLimits,
    ) -> Self {
        let count = |key| {
            let bound = limits.get(key);
            if bound.unlimited {
                usize::MAX
            } else {
                usize::try_from(bound.value).unwrap_or(0)
            }
        };
        self.limits.max_depth = count("chain.max_depth");
        self.limits.max_total = count("chain.max_requests");
        self
    }

    pub(super) fn apply_websocket_environment_mutations(
        &mut self,
        mutations: &[EnvironmentMutation],
    ) -> Result<Option<Environment>, String> {
        let Some(id) = self.environment_id.as_deref() else {
            return Ok(None);
        };
        crate::core::apply_environment_mutations_to_workspace(&mut self.workspace, id, mutations)?;
        Ok(self.workspace.environment(id).cloned())
    }

    /// Each WebSocket event owns an independent chain budget and cancellation.
    pub(super) fn for_websocket_event(&self) -> Self {
        let mut runner = self.clone();
        runner.budget = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        runner.chain_cancellation = ScriptCancellation::new();
        runner
    }
}

impl crate::core::InlineChainer for InlineChainRunner {
    fn run_at_depth(
        &self,
        requested: &[crate::core::ChainedRequest],
        depth: usize,
    ) -> Vec<Result<crate::core::ChainRun, String>> {
        self.run_with_budget(requested, depth, None)
    }

    fn run(
        &self,
        requested: &[crate::core::ChainedRequest],
    ) -> Vec<Result<crate::core::ChainRun, String>> {
        self.run_with_budget(requested, 0, None)
    }

    fn run_with_budget(
        &self,
        requested: &[crate::core::ChainedRequest],
        depth: usize,
        deadline: Option<std::time::Instant>,
    ) -> Vec<Result<crate::core::ChainRun, String>> {
        let mut nested = self.clone();
        nested.limits.max_depth = nested.limits.max_depth.saturating_sub(depth);
        nested.chain_cancellation = self.chain_cancellation.child();
        nested.run_batch(requested, deadline)
    }
}

impl InlineChainRunner {
    fn run_batch(
        &self,
        requested: &[crate::core::ChainedRequest],
        deadline: Option<std::time::Instant>,
    ) -> Vec<Result<crate::core::ChainRun, String>> {
        let futures = requested
            .iter()
            .map(|scheduled| {
                let workspace = self.workspace.clone();
                let namespace = self.namespace.clone();
                let environment_id = self.environment_id.clone();
                let chain_cancellation = self.chain_cancellation.clone();
                let budget = self.budget.clone();
                let limits = self.limits;
                let chain_inline: &dyn crate::core::InlineChainer = self;
                let requested = vec![scheduled.clone()];
                async move {
                    let run = crate::core::run_chain_with_executor(
                        &workspace,
                        environment_id.as_deref(),
                        &requested,
                        &namespace,
                        &chain_cancellation,
                        Some(chain_inline),
                        self,
                        limits,
                        &budget,
                    )
                    .await;
                    if let Some(error) = run.error.as_ref() {
                        Err(error.message.clone())
                    } else {
                        Ok(run)
                    }
                }
            })
            .collect::<Vec<_>>();

        // Drive every awaited pipeline concurrently on the shared runtime, but
        // from a fresh scoped thread whose Tokio context is clear: a chained
        // request's own script that `await execute(...)` re-enters this runner
        // from within the enclosing runtime, so `block_on` must happen on a
        // thread with no active runtime context (avoids "runtime within a
        // runtime"). reqwest multiplexes all in-flight network calls.
        let failure = match crate::core::drive_inline_with_budget(
            &self.runtime,
            &self.chain_cancellation,
            deadline,
            futures::future::join_all(futures),
        ) {
            Ok(Some(outcomes)) => return outcomes,
            Ok(None) => "awaited request execution timed out",
            Err(_) => "awaited request execution thread panicked",
        };
        requested.iter().map(|_| Err(failure.to_owned())).collect()
    }
}

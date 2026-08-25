use super::*;
use std::sync::atomic::Ordering;

// Script errors intentionally carry a full structured report and diagnostic.
// Keeping the unboxed error preserves that context across the background task.
#[allow(clippy::result_large_err)]
impl ApiTester {
    fn script_scope(environment: Option<&Environment>) -> ScriptScope {
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
        let template = self.request_template(cx);
        let environment_id = self.workspace.active_environment_id.clone();
        let mut scope = Self::script_scope(
            environment_id
                .as_deref()
                .and_then(|id| self.workspace.environment(id)),
        );
        scope.script_timeout = self.settings.script.timeout();
        self.request_generation = self.request_generation.wrapping_add(1);
        let generation = self.request_generation;
        self.request_history_target = self.active_upstream_workspace().ok();
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
        let task = self.runtime.spawn_blocking(move || {
            crate::core::execute_pre_request_with_chain(
                &source,
                &request,
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
                this.finish_pre_request(generation, template, environment_id, result, window, cx);
            });
        })
        .detach();
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
        let local_client = self.client.clone();
        let upstream_client = self.upstream_execution_client.clone();
        let target = match self.workspace_providers.active_id() {
            WorkspaceProviderId::Local(_) => None,
            WorkspaceProviderId::Upstream { .. } => match self.active_upstream_workspace() {
                Ok(target) => Some((target.upstream_id, target.workspace_id, target.base_url)),
                Err(error) => {
                    self.fail_request(
                        &crate::core::RequestDraft::new("GET", ""),
                        format!("Could not prepare chained execution: {error}"),
                        cx,
                    );
                    return;
                }
            },
        };
        let vault = self.credential_vault.clone();
        let runtime = Arc::clone(&self.runtime);
        let mut limits = crate::core::ChainLimits::default();
        limits.script_timeout = self.settings.script.timeout();

        let task = self.runtime.spawn(async move {
            let sender = move |request: crate::core::RequestDraft| {
                let local_client = local_client.clone();
                let upstream_client = upstream_client.clone();
                let vault = vault.clone();
                let runtime = Arc::clone(&runtime);
                let target = target.clone();
                async move {
                    match target {
                        None => crate::core::send_request(&local_client, request).await,
                        Some((upstream_id, workspace_id, base_url)) => {
                            let credential = runtime
                                .spawn_blocking(move || vault.load_upstream(&upstream_id))
                                .await
                                .map_err(|error| {
                                    crate::core::RequestError::TaskFailed(error.to_string())
                                })?
                                .map_err(|error| {
                                    crate::core::RequestError::Upstream(error.to_string())
                                })?
                                .ok_or_else(|| {
                                    crate::core::RequestError::Upstream(
                                        "Log in to this server again.".to_owned(),
                                    )
                                })?;
                            if credential.expires_at <= Utc::now() {
                                return Err(crate::core::RequestError::Upstream(
                                    "Log in to this server again.".to_owned(),
                                ));
                            }
                            crate::core::send_request_for_upstream_workspace(
                                &upstream_client,
                                &local_client,
                                &base_url,
                                credential.bearer_token(),
                                &workspace_id,
                                request,
                            )
                            .await
                        }
                    }
                }
            };
            crate::core::run_chain(
                &workspace,
                environment_id.as_deref(),
                &scheduled,
                &namespace,
                &chain_cancellation,
                None,
                sender,
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
    fn build_inline_chainer(&self, environment_id: &Option<String>) -> InlineChainRunner {
        let workspace = self.workspace.clone();
        let namespace = self.request_namespace.clone();
        let environment_id = environment_id.clone();
        let chain_cancellation = self
            .script_cancellation
            .clone()
            .unwrap_or_else(crate::core::ScriptCancellation::new);
        let budget = Arc::clone(&self.chain_budget);
        let local_client = self.client.clone();
        let upstream_client = self.upstream_execution_client.clone();
        let vault = self.credential_vault.clone();
        let runtime = Arc::clone(&self.runtime);
        let target = match self.workspace_providers.active_id() {
            WorkspaceProviderId::Local(_) => None,
            WorkspaceProviderId::Upstream { .. } => match self.active_upstream_workspace() {
                Ok(target) => Some((target.upstream_id, target.workspace_id, target.base_url)),
                Err(_) => None,
            },
        };
        let mut limits = crate::core::ChainLimits::default();
        limits.script_timeout = self.settings.script.timeout();
        InlineChainRunner {
            workspace,
            namespace,
            environment_id,
            chain_cancellation,
            budget,
            local_client,
            upstream_client,
            vault,
            runtime,
            target,
            limits,
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
        if !run.environment_mutations.is_empty() {
            if let Some(environment_id) = self.workspace.active_environment_id.clone() {
                let _ = self.apply_environment_mutations(
                    Some(&environment_id),
                    &run.environment_mutations,
                    window,
                    cx,
                );
            }
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
        let task: RequestTask = match self.workspace_providers.active_id() {
            WorkspaceProviderId::Local(_) => spawn_request(
                self.runtime.handle(),
                self.client.clone(),
                resolved.request.clone(),
            ),
            WorkspaceProviderId::Upstream { .. } => {
                let target = match self.active_upstream_workspace() {
                    Ok(target) => target,
                    Err(error) => {
                        self.pre_script_report = Some(pre_report);
                        self.fail_request_with_secrets(
                            &resolved.request,
                            resolved.redact_secrets(&error),
                            &resolved.sensitive_values,
                            cx,
                        );
                        return;
                    }
                };
                let vault = self.credential_vault.clone();
                let client = self.upstream_execution_client.clone();
                let local_client = self.client.clone();
                let runtime = Arc::clone(&self.runtime);
                let credential_upstream_id = target.upstream_id.clone();
                let request = resolved.request.clone();
                RequestTask::spawn(self.runtime.handle(), async move {
                    let credential = runtime
                        .spawn_blocking(move || vault.load_upstream(&credential_upstream_id))
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
                    send_request_for_upstream_workspace(
                        &client,
                        &local_client,
                        &target.base_url,
                        credential.bearer_token(),
                        &target.workspace_id,
                        request,
                    )
                    .await
                })
            }
        };
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
            self.run_post_chain(generation, environment_id, post_chained, window, cx);
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
        cx.notify();
    }

    fn run_post_chain(
        &mut self,
        generation: u64,
        environment_id: Option<String>,
        post_chained: Vec<crate::core::ChainedRequest>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.spawn_chain(
            generation,
            environment_id,
            post_chained,
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
        self.request_generation = self.request_generation.wrapping_add(1);
        self.finish_cancelled(cx);
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
        cx.notify();
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

struct InlineChainRunner {
    workspace: crate::core::Workspace,
    namespace: crate::core::RequestNamespaceCatalog,
    environment_id: Option<String>,
    chain_cancellation: crate::core::ScriptCancellation,
    budget: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    local_client: reqwest::Client,
    upstream_client: reqwest::Client,
    vault: crate::core::CredentialVault,
    runtime: std::sync::Arc<tokio::runtime::Runtime>,
    target: Option<(String, String, reqwest::Url)>,
    limits: crate::core::ChainLimits,
}

impl crate::core::InlineChainer for InlineChainRunner {
    fn run(
        &self,
        requested: &[crate::core::ChainedRequest],
    ) -> Vec<Result<crate::core::ChainRun, String>> {
        let futures = requested
            .iter()
            .map(|scheduled| {
                let workspace = self.workspace.clone();
                let namespace = self.namespace.clone();
                let environment_id = self.environment_id.clone();
                let chain_cancellation = self.chain_cancellation.clone();
                let budget = self.budget.clone();
                let local_client = self.local_client.clone();
                let upstream_client = self.upstream_client.clone();
                let vault = self.vault.clone();
                let runtime = self.runtime.clone();
                let target = self.target.clone();
                let limits = self.limits;
                let script_timeout = self.limits.script_timeout;
                let chain_inline: &dyn crate::core::InlineChainer = self;
                let requested = vec![scheduled.clone()];
                async move {
                    let sender = move |request: crate::core::RequestDraft| {
                        let local_client = local_client.clone();
                        let upstream_client = upstream_client.clone();
                        let vault = vault.clone();
                        let runtime = runtime.clone();
                        let target = target.clone();
                        async move {
                            match target {
                                None => crate::core::send_request(&local_client, request).await,
                                Some((upstream_id, workspace_id, base_url)) => {
                                    let credential = runtime
                                        .spawn_blocking(move || vault.load_upstream(&upstream_id))
                                        .await
                                        .map_err(|error| {
                                            crate::core::RequestError::TaskFailed(error.to_string())
                                        })?
                                        .map_err(|error| {
                                            crate::core::RequestError::Upstream(error.to_string())
                                        })?
                                        .ok_or_else(|| {
                                            crate::core::RequestError::Upstream(
                                                "Log in to this server again.".to_owned(),
                                            )
                                        })?;
                                    if credential.expires_at <= Utc::now() {
                                        return Err(crate::core::RequestError::Upstream(
                                            "Log in to this server again.".to_owned(),
                                        ));
                                    }
                                    crate::core::send_request_for_upstream_workspace(
                                        &upstream_client,
                                        &local_client,
                                        &base_url,
                                        credential.bearer_token(),
                                        &workspace_id,
                                        request,
                                    )
                                    .await
                                }
                            }
                        }
                    };
                    match tokio::time::timeout(
                        script_timeout,
                        crate::core::run_chain(
                            &workspace,
                            environment_id.as_deref(),
                            &requested,
                            &namespace,
                            &chain_cancellation,
                            Some(chain_inline),
                            sender,
                            limits,
                            &budget,
                        ),
                    )
                    .await
                    {
                        Ok(run) if run.error.is_none() => Ok(run),
                        Ok(run) => Err(run.error.unwrap().message),
                        Err(_) => Err("awaited request exceeded its execution limit".to_owned()),
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
        std::thread::scope(|scope| {
            scope
                .spawn(move || {
                    self.runtime
                        .block_on(async { futures::future::join_all(futures).await })
                })
                .join()
                .unwrap_or_else(|_| {
                    requested
                        .iter()
                        .map(|_| Err("awaited request execution thread panicked".to_owned()))
                        .collect()
                })
        })
    }
}
